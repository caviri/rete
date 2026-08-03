# Graph algorithms over `.rete`

A `.rete` file is a graph — so everything the network-science toolbox knows how
to do (centrality, community detection, similarity, path finding, embeddings,
link prediction, graph ML) applies to it. This page is the catalog: which
algorithm lives in which Python library, how each library connects to a
`.rete`, and what the engine already computes natively.

The pattern is always the same three steps:

1. **Project** — a SPARQL query pulls the sub-graph you care about out of the
   file as an edge list. Opens are lazy, so only the byte ranges the query
   touches are ever downloaded — projecting from a multi-GB remote file costs
   megabytes, not the file.
2. **Compute** — the edge list feeds NetworkX, SciPy, scikit-learn, and friends.
   All of the "browser" rows below run in Pyodide, so the whole loop works
   inside a browser tab.
3. **Write back** — results become new triples in a *derived* `.rete` built
   with the `Builder`: your analytics end up as versioned, queryable,
   self-describing RDF instead of a throwaway dict.

The worked example for everything on this page is the
**[graph data science notebook](jupyterlite/lab/index.html?path=graph-data-science.ipynb)**
(JupyterLite — nothing to install), which runs the full catalog against the
[Mark Lombardi networks](lombardi-guide.md) dataset. A minimal projection:

```python
import rete_graph as rete
import networkx as nx

g = rete.open("https://data.graphplaza.com/lombardi/lombardi.rete")
edges = g.query_df("""
    PREFIX lomb: <https://w3id.org/rete/lombardi/>
    SELECT ?src ?tgt ?type WHERE {
      ?arc lomb:source ?src ; lomb:target ?tgt ; lomb:arcType ?type .
      FILTER(?src != ?tgt)
    }""")

G = nx.from_pandas_edgelist(edges, "src", "tgt")   # and compute away
```

Projection is also where *filtering* happens — `FILTER`, `VALUES` and
`FILTER NOT EXISTS` restrict relationship types and node classes before a
single algorithm runs. And because a `.rete` can carry its own OWL ontology,
`reason=True` turns the class/property hierarchy into projection config: ask
for a super-property and entailment widens the edge set with every declared
sub-property, no re-projection needed (see [Reasoning](reasoning.md)).

## What the engine does natively — no Python required

Some graph analytics never need an export at all:

| Task | How |
|---|---|
| Degree centrality | plain SPARQL `GROUP BY` + `COUNT` |
| Reachability / transitive closure | SPARQL property paths — `(p\|^p)+` from a bound endpoint |
| Community detection (Louvain) | `rete communities` — the same partition that builds the file's [pyramid summary](intro.md) |
| Community labelling | `rete communities --profile` — top words, classes, predicates per community |
| Class/link structure | the embedded [schema profile](dataset-cards.md) — classes, instance counts, class-to-class relations |
| Full-text candidate search | the [text index](sparql.md) (`--text-index` builds, `text_search()` queries) |
| Ontology entailment | OWL 2 QL by query rewriting — `reason=True` / `--entail` |

For latent-theme labelling on top of the native communities, see
[Topic modeling (LDA)](topic-modeling.md); for partitions that combine
structure with attributes, see [Multi-criteria communities](multi-criteria.md).

## The algorithm catalog

"Browser" means the library is pure Python or ships Pyodide wheels, so the row
runs in a tab (JupyterLite/marimo) as well as natively. "Native" means CPython
only — same projection code, run it locally.

### Centrality

| Algorithm | Call | Runs |
|---|---|---|
| Degree | SPARQL `GROUP BY` — or `G.degree` | engine / browser |
| PageRank | `nx.pagerank` | browser |
| Betweenness (exact or sampled) | `nx.betweenness_centrality(k=…)` | browser |
| Closeness / Harmonic | `nx.closeness_centrality`, `nx.harmonic_centrality` | browser |
| Eigenvector | `nx.eigenvector_centrality` | browser |
| Katz | `nx.katz_centrality` | browser |
| HITS (hubs & authorities) | `nx.hits` | browser |
| Articulation points / Bridges | `nx.articulation_points`, `nx.bridges` | browser |

### Community detection

| Algorithm | Call | Runs |
|---|---|---|
| Louvain | `nx.community.louvain_communities` — or `rete communities` | browser / engine |
| Leiden | `sknetwork.clustering.Leiden`, `leidenalg` (igraph) | native |
| Label propagation | `nx.community.label_propagation_communities` | browser |
| Greedy modularity | `nx.community.greedy_modularity_communities` | browser |
| Connected components (weak/strong) | `nx.connected_components`, `nx.strongly_connected_components` | browser |
| Triangle count / clustering coefficient | `nx.triangles`, `nx.average_clustering` | browser |
| K-core decomposition | `nx.core_number` | browser |
| Graph coloring | `nx.greedy_color` | browser |
| Modularity / conductance metrics | `nx.community.modularity`, `nx.conductance` | browser |
| K-Means / HDBSCAN on embeddings | `sklearn.cluster` | browser |

### Similarity & nearest neighbours

| Algorithm | Call | Runs |
|---|---|---|
| Jaccard / overlap node similarity | set ops over neighbourhoods | browser |
| Cosine / Euclidean similarity | `scipy.spatial.distance` | browser |
| K-nearest neighbours (on embeddings) | `sklearn.neighbors.NearestNeighbors` | browser |

### Path finding

| Algorithm | Call | Runs |
|---|---|---|
| Shortest path (Dijkstra) | `nx.shortest_path`, `nx.single_source_dijkstra` | browser |
| A* | `nx.astar_path` | browser |
| k-shortest simple paths | `nx.shortest_simple_paths` | browser |
| Bellman-Ford | `nx.bellman_ford_path` | browser |
| BFS / DFS | `nx.bfs_tree`, `nx.dfs_tree` | browser |
| All-pairs shortest paths | `nx.all_pairs_shortest_path_length` | browser |
| Minimum spanning tree | `nx.minimum_spanning_tree` | browser |
| Steiner tree (approx.) | `nx.algorithms.approximation.steiner_tree` | browser |
| Max-flow / min-cost flow | `nx.maximum_flow`, `nx.min_cost_flow` | browser |
| Topological sort / DAG longest path | `nx.topological_sort`, `nx.dag_longest_path` | browser |
| Random walks | a few lines of NumPy over the CSR matrix | browser |
| Reachability | SPARQL property paths — no export | engine |

### Node embeddings

| Algorithm | Call | Runs |
|---|---|---|
| Fast random projection (FastRP) | ~15 lines of NumPy — sparse random projection + powers of the normalized adjacency (in the notebook) | browser |
| Spectral embedding | `sklearn.manifold.SpectralEmbedding`, `sknetwork.embedding` | browser / native |
| node2vec | random walks + `gensim` word2vec | native |
| GraphSAGE / GCN / GAT … | PyTorch Geometric | native |
| 2-D layout for plots | `sklearn.decomposition.PCA`, UMAP | browser / native |

### Link prediction

| Algorithm | Call | Runs |
|---|---|---|
| Adamic-Adar | `nx.adamic_adar_index` | browser |
| Common neighbours | `nx.common_neighbors` | browser |
| Preferential attachment | `nx.preferential_attachment` | browser |
| Resource allocation | `nx.resource_allocation_index` | browser |
| Learned link prediction | `sklearn` classifier on pair features, or PyG | browser / native |

Restrict candidates to distance-2 pairs (nodes sharing a neighbour) — scoring
all non-edges is quadratic and almost never what you want.

### Graph machine learning

| Task | Call | Runs |
|---|---|---|
| Node classification / regression | `sklearn` (logistic regression, random forest, MLP) on typed-edge counts + embeddings | browser |
| Train/test discipline | `sklearn.model_selection` | browser |
| GNN pipelines | PyTorch Geometric (`Data`, `HeteroData`, `GCNConv`, …) | native |

One finding from the worked example worth internalizing: on typed RDF graphs,
**the predicates are the features**. Counting a node's incident edges *by
predicate* beat both pure-structure embeddings and a 2-layer GCN for class
prediction — the typed edges that a property-graph projection flattens away
are first-class signal here.

## The library connections

**pandas** — `query_df()` returns projections as DataFrames; every bridge
below starts from one. Install: `pip install rete-graph[pandas]`.

**NetworkX** — the default compute engine: pure Python, complete algorithm
coverage, runs in Pyodide. `nx.from_pandas_edgelist(edges, "src", "tgt")`, or
add edges in a loop when you want multiplicity → weights.

**SciPy sparse** — the lingua franca. Build a CSR adjacency once and it feeds
FastRP (three `@` products), spectral methods, and scikit-network unchanged:

```python
import numpy as np, scipy.sparse as sp
nodes = pd.unique(pd.concat([edges["src"], edges["tgt"]]))
ix = {n: i for i, n in enumerate(nodes)}
r = edges["src"].map(ix); c = edges["tgt"].map(ix)
A = sp.csr_matrix((np.ones(len(edges)), (r, c)), shape=(len(nodes),) * 2)
A = A + A.T                                       # undirected
```

**scikit-learn** — everything downstream of a matrix: kNN, K-Means/HDBSCAN,
PCA, classifiers, cross-validation. In Pyodide out of the box.

**scikit-network** — compiled Louvain/Leiden/PageRank/embeddings that consume
exactly the CSR above (`Louvain().fit_predict(A)`), orders of magnitude faster
than NetworkX. Native only — reach for it past ~10⁶ edges.

**PyTorch Geometric** — the GNN bridge. `edge_index` is the projection's two
columns factorized; and `HeteroData` is *shaped like RDF*: one relation per
predicate.

```python
import torch
from torch_geometric.data import Data, HeteroData

ei = torch.tensor([edges["src"].map(ix).values,
                   edges["tgt"].map(ix).values], dtype=torch.long)
data = Data(edge_index=ei, num_nodes=len(nodes))

hetero = HeteroData()
hetero["actor"].num_nodes = len(nodes)
for t, sub in edges.groupby("type"):              # one relation per predicate
    hetero["actor", t.split("#")[-1], "actor"].edge_index = torch.tensor(
        [sub["src"].map(ix).values, sub["tgt"].map(ix).values])
```

Node features come from the graph too: literal values, class one-hots, or the
per-predicate incidence counts that worked so well above. Native only (no
Pyodide torch); train GCN/GraphSAGE/GAT as usual from there.

**igraph / leidenalg, gensim** — Leiden partitions and word2vec-style node2vec
respectively; both native, both fed by the same edge list.

## Scale honesty

- **In a browser tab**: NetworkX is comfortable to ~10⁵–10⁶ edges. Lazy reads
  mean the download is bounded by the *projection*, not the file.
- **Natively**: the same code, then scikit-network / igraph / PyG for the
  heavy lifting. Still one `pip install`, still no database server.
- **Beyond**: published datasets ship [Parquet companions](media-companions.md)
  for columnar bulk analytics, and any `.rete` URL is a standard
  [SPARQL endpoint](interop.md) for server-side tooling.
- **Write-back**: derive a new `.rete` with the results (`Builder`), so scores
  and partitions become citable data with provenance — see the final section
  of the notebook.

## Where to start

- **[The notebook](jupyterlite/lab/index.html?path=graph-data-science.ipynb)** —
  every family above, live in your browser, on a real dataset.
- [JupyterLite overview](jupyterlite-guide.md) · [Python API](python.md) ·
  [SPARQL dialect](sparql.md) · [Reasoning](reasoning.md) ·
  [Topic modeling](topic-modeling.md) ·
  [Multi-criteria communities](multi-criteria.md)

Source & issues: <https://github.com/caviri/rete> · © 2026 Carlos Vivar Ríos,
released under the
[Apache License 2.0](https://github.com/caviri/rete/blob/main/LICENSE).
