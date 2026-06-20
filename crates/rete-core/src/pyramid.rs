//! Community detection and quotient-graph coarsening — the engine behind the
//! size-targeted pyramid (SPEC.md §7).
//!
//! This module is deliberately graph-only: it works on an abstract weighted
//! undirected graph of node IDs `0..n` and knows nothing about RDF terms or
//! tiles. The pyramid builder (a later layer) projects the triples onto this
//! graph, runs [`louvain_one_level`] repeatedly — coarsening via
//! [`Graph::quotient`] between rounds — until a level's tiles fit the byte
//! budget, then materializes per-community triple tiles.

use std::collections::HashMap;

use crate::dictionary::Dictionary;

/// A weighted undirected graph over nodes `0..n`. Parallel edges are summed;
/// self-loops are kept (they affect modularity but not community moves).
#[derive(Debug, Clone)]
pub struct Graph {
    n: usize,
    /// `adj[i]` = list of `(neighbor, weight)`.
    adj: Vec<Vec<(usize, f64)>>,
    /// Weighted degree per node (self-loops counted twice).
    degree: Vec<f64>,
    /// Total edge weight `m` (each undirected edge contributes its weight once).
    m: f64,
}

impl Graph {
    /// Build from undirected weighted edges. `(u, v, w)` and `(v, u, w)` are
    /// equivalent; parallel edges accumulate.
    pub fn from_edges(n: usize, edges: &[(usize, usize, f64)]) -> Self {
        let mut adj_w: Vec<HashMap<usize, f64>> = vec![HashMap::new(); n];
        let mut degree = vec![0.0; n];
        let mut m = 0.0;
        for &(u, v, w) in edges {
            assert!(u < n && v < n, "edge endpoint out of range");
            *adj_w[u].entry(v).or_insert(0.0) += w;
            degree[u] += w;
            m += w;
            if u != v {
                *adj_w[v].entry(u).or_insert(0.0) += w;
                degree[v] += w;
            } else {
                // self-loop adds twice to degree.
                degree[u] += w;
            }
        }
        // Sort each node's neighbour list by index: a canonical adjacency order
        // so every downstream traversal (gain sums, modularity, quotient) is
        // order-stable — floating-point addition is not associative, so a
        // randomised neighbour order would otherwise perturb sums in the low
        // bits and make the whole pyramid (and the file's content hash)
        // non-reproducible.
        let adj = adj_w
            .into_iter()
            .map(|h| {
                let mut nbrs: Vec<(usize, f64)> = h.into_iter().collect();
                nbrs.sort_unstable_by_key(|&(j, _)| j);
                nbrs
            })
            .collect();
        Graph { n, adj, degree, m }
    }

    pub fn node_count(&self) -> usize {
        self.n
    }

    pub fn total_weight(&self) -> f64 {
        self.m
    }

    /// Newman modularity of a partition (community id per node).
    pub fn modularity(&self, comm: &[usize]) -> f64 {
        if self.m == 0.0 {
            return 0.0;
        }
        let two_m = 2.0 * self.m;
        let mut q = 0.0;
        // sum over edges of [A_ij - k_i k_j / 2m] within same community.
        for (i, neighbors) in self.adj.iter().enumerate() {
            for &(j, w) in neighbors {
                if comm[i] == comm[j] {
                    q += w; // A_ij summed over ordered pairs (both directions present)
                }
            }
        }
        // subtract degree term, summed per community.
        let mut sigma_tot: HashMap<usize, f64> = HashMap::new();
        for (i, &c) in comm.iter().enumerate() {
            *sigma_tot.entry(c).or_insert(0.0) += self.degree[i];
        }
        let deg_term: f64 = sigma_tot.values().map(|&s| s * s).sum();
        (q - deg_term / two_m) / two_m
    }

    /// Coarsen the graph: each community becomes one node. Returns the quotient
    /// graph and the dense community count. `comm` must use dense IDs `0..k`.
    pub fn quotient(&self, comm: &[usize], k: usize) -> Graph {
        let mut edges: HashMap<(usize, usize), f64> = HashMap::new();
        for (i, neighbors) in self.adj.iter().enumerate() {
            for &(j, w) in neighbors {
                if i <= j {
                    let (ci, cj) = (comm[i], comm[j]);
                    let key = (ci.min(cj), ci.max(cj));
                    *edges.entry(key).or_insert(0.0) += w;
                }
            }
        }
        // Canonical edge order → deterministic accumulation in `from_edges`.
        let mut edge_vec: Vec<(usize, usize, f64)> =
            edges.into_iter().map(|((a, b), w)| (a, b, w)).collect();
        edge_vec.sort_unstable_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        Graph::from_edges(k, &edge_vec)
    }
}

/// A community partition with dense IDs `0..count`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    /// Community ID per node.
    pub comm: Vec<usize>,
    /// Number of distinct communities.
    pub count: usize,
}

/// One level of Louvain local-moving modularity optimization.
///
/// Nodes start in their own community; each is greedily moved to the neighboring
/// community giving the best modularity gain until no move improves. IDs in the
/// result are renumbered densely.
pub fn louvain_one_level(g: &Graph) -> Partition {
    let n = g.n;
    if g.m == 0.0 {
        return Partition {
            comm: (0..n).collect(),
            count: n,
        };
    }
    let two_m = 2.0 * g.m;
    let mut comm: Vec<usize> = (0..n).collect();
    let mut sigma_tot: Vec<f64> = g.degree.clone(); // each node alone

    // Reusable scratch (allocated once, not per node): `k_to[c]` is the edge weight
    // from the current node `i` into community `c`, valid only for the communities
    // recorded in `touched`. A dense array + touched-list replaces the per-node
    // `HashMap` the textbook formulation allocates — same accumulation order (the
    // adjacency is index-sorted, so the float sums are bit-identical) and the same
    // sorted candidate scan, so the partition is byte-for-byte unchanged. Edge
    // weights are strictly positive, so `k_to[c] == 0.0` reliably means "untouched".
    let mut k_to = vec![0.0f64; n];
    let mut touched: Vec<usize> = Vec::new();

    let mut improved = true;
    while improved {
        improved = false;
        for (i, neighbors) in g.adj.iter().enumerate() {
            let ci = comm[i];
            let ki = g.degree[i];

            // Tentatively remove i from its community.
            sigma_tot[ci] -= ki;

            // Edge weight from i into each neighboring community (into the dense
            // scratch, recording first-touch communities for O(touched) reset).
            for &(j, w) in neighbors {
                if j != i {
                    let cj = comm[j];
                    if k_to[cj] == 0.0 {
                        touched.push(cj);
                    }
                    k_to[cj] += w;
                }
            }

            // Gain of placing i into community c: k_{i,c} - sigma_tot[c]*ki/2m.
            let gain = |c: usize| -> f64 { k_to[c] - sigma_tot[c] * ki / two_m };
            // Scan candidate communities in a deterministic (sorted) order so an
            // equal-gain tie always resolves to the same community across runs —
            // the second half of making the pyramid reproducible. Strict `>`
            // keeps the first (smallest-id) community on ties.
            let mut best_c = ci;
            let mut best_gain = gain(ci);
            touched.sort_unstable();
            for &c in &touched {
                let gc = gain(c);
                if gc > best_gain {
                    best_gain = gc;
                    best_c = c;
                }
            }

            sigma_tot[best_c] += ki;
            comm[i] = best_c;
            if best_c != ci {
                improved = true;
            }

            // Reset only the touched entries for the next node.
            for &c in &touched {
                k_to[c] = 0.0;
            }
            touched.clear();
        }
    }

    // Renumber densely.
    let mut remap: HashMap<usize, usize> = HashMap::new();
    for c in &mut comm {
        let next = remap.len();
        *c = *remap.entry(*c).or_insert(next);
    }
    Partition {
        count: remap.len(),
        comm,
    }
}

/// Project RDF integer triples onto the undirected node graph the pyramid
/// clusters: one node per distinct term (via the dictionary's unified node
/// space), one unit-weight edge per `(subject, object)` pair (predicates are
/// ignored for clustering; parallel edges accumulate weight).
pub fn project_graph(dict: &Dictionary, triples: &[(u32, u32, u32)]) -> Graph {
    let n = dict.node_count() as usize;
    let edges: Vec<(usize, usize, f64)> = triples
        .iter()
        .map(|&(s, _, o)| {
            (
                dict.subject_node(s) as usize,
                dict.object_node(o) as usize,
                1.0,
            )
        })
        .collect();
    Graph::from_edges(n, &edges)
}

/// A hierarchy of community partitions (a dendrogram). `levels[0]` partitions the
/// base nodes; `levels[k]` partitions level `k-1`'s communities. Round 0 is the
/// finest grouping; the last round is the coarsest (pyramid level 0).
#[derive(Debug, Clone)]
pub struct Dendrogram {
    pub levels: Vec<Partition>,
}

impl Dendrogram {
    /// Number of coarsening rounds (0 if the graph had no community structure).
    pub fn rounds(&self) -> usize {
        self.levels.len()
    }

    /// Distinct community count at `round`.
    pub fn community_count(&self, round: usize) -> usize {
        self.levels[round].count
    }

    /// The community a base `node` belongs to at the given `round`, composing
    /// the partitions `levels[0..=round]`.
    pub fn base_community(&self, node: usize, round: usize) -> usize {
        let mut c = node;
        for level in &self.levels[..=round] {
            c = level.comm[c];
        }
        c
    }
}

/// Build the full dendrogram by repeated Louvain + coarsening, stopping when a
/// round no longer compresses the graph (or only one node remains).
pub fn build_dendrogram(g: &Graph) -> Dendrogram {
    let mut levels = Vec::new();
    let mut current = g.clone();
    loop {
        let p = louvain_one_level(&current);
        if p.count >= current.node_count() {
            break; // no compression — stop
        }
        let next = current.quotient(&p.comm, p.count);
        levels.push(p);
        if next.node_count() <= 1 {
            break;
        }
        current = next;
    }
    Dendrogram { levels }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::DictionaryBuilder;

    /// Two triangles {0,1,2} and {3,4,5} joined by a single 3–? bridge.
    fn barbell() -> Graph {
        let edges = [
            (0, 1, 1.0),
            (1, 2, 1.0),
            (0, 2, 1.0),
            (3, 4, 1.0),
            (4, 5, 1.0),
            (3, 5, 1.0),
            (2, 3, 1.0), // bridge
        ];
        Graph::from_edges(6, &edges)
    }

    #[test]
    fn two_communities() {
        let g = barbell();
        let p = louvain_one_level(&g);
        assert_eq!(p.count, 2, "barbell should split into two communities");
        // Nodes 0,1,2 together; 3,4,5 together.
        assert_eq!(p.comm[0], p.comm[1]);
        assert_eq!(p.comm[1], p.comm[2]);
        assert_eq!(p.comm[3], p.comm[4]);
        assert_eq!(p.comm[4], p.comm[5]);
        assert_ne!(p.comm[2], p.comm[3]);
        // Modularity of the found partition beats the all-in-one partition.
        assert!(g.modularity(&p.comm) > g.modularity(&[0; 6]));
    }

    #[test]
    fn dendrogram_is_deterministic_across_runs() {
        // A tie-prone graph: six 6-cliques in a ring, joined by single bridges,
        // so many local moves have equal-gain candidates. Two builds in the same
        // process use different HashMap seeds — they must still agree exactly, or
        // the pyramid (and the file's content hash) is not reproducible.
        let mut edges = Vec::new();
        let clusters = 6;
        let size = 6;
        for c in 0..clusters {
            let base = c * size;
            for i in 0..size {
                for j in (i + 1)..size {
                    edges.push((base + i, base + j, 1.0));
                }
            }
            // bridge to the next cluster (ring).
            let next = ((c + 1) % clusters) * size;
            edges.push((base, next, 1.0));
        }
        let g = Graph::from_edges(clusters * size, &edges);
        let a = build_dendrogram(&g);
        let b = build_dendrogram(&g);
        assert_eq!(a.levels, b.levels, "dendrogram must be reproducible");
        assert!(!a.levels.is_empty(), "the ring of cliques should compress");
    }

    #[test]
    fn quotient_collapses_communities() {
        let g = barbell();
        let p = louvain_one_level(&g);
        let q = g.quotient(&p.comm, p.count);
        assert_eq!(q.node_count(), 2);
        // Total weight is preserved by coarsening.
        assert!((q.total_weight() - g.total_weight()).abs() < 1e-9);
    }

    #[test]
    fn empty_graph_is_singletons() {
        let g = Graph::from_edges(3, &[]);
        let p = louvain_one_level(&g);
        assert_eq!(p.count, 3);
    }

    #[test]
    fn dendrogram_groups_barbell() {
        let g = barbell();
        let d = build_dendrogram(&g);
        assert!(d.rounds() >= 1);
        // At the finest round, the two triangles are distinct communities.
        let c0 = d.base_community(0, 0);
        assert_eq!(c0, d.base_community(1, 0));
        assert_eq!(c0, d.base_community(2, 0));
        let c3 = d.base_community(3, 0);
        assert_eq!(c3, d.base_community(4, 0));
        assert_ne!(c0, c3);
    }

    /// Two fully-connected triples-clusters joined by one bridge edge, expressed
    /// as RDF `knows` triples, then projected and clustered.
    #[test]
    fn project_and_cluster_rdf() {
        let edges = [
            ("A", "B"),
            ("B", "C"),
            ("A", "C"),
            ("D", "E"),
            ("E", "F"),
            ("D", "F"),
            ("C", "D"), // bridge
        ];
        let mut db = DictionaryBuilder::new();
        for (s, o) in edges {
            db.observe(s, "knows", o);
        }
        let dict = db.build();
        let triples: Vec<_> = edges
            .iter()
            .map(|(s, o)| dict.encode(s, "knows", o).unwrap())
            .collect();

        let g = project_graph(&dict, &triples);
        assert_eq!(g.node_count(), 6); // A..F all shared nodes
        let d = build_dendrogram(&g);

        // A is subject-only, F is object-only — resolve a node via either role.
        let node = |t: &str| {
            dict.subject_id(t)
                .map(|id| dict.subject_node(id))
                .or_else(|| dict.object_id(t).map(|id| dict.object_node(id)))
                .unwrap() as usize
        };
        let comm = |t: &str| d.base_community(node(t), 0);
        // A, B, C cluster together; D, E, F cluster together; clusters differ.
        assert_eq!(comm("A"), comm("B"));
        assert_eq!(comm("B"), comm("C"));
        assert_eq!(comm("D"), comm("E"));
        assert_eq!(comm("E"), comm("F"));
        assert_ne!(comm("C"), comm("D"));
    }
}
