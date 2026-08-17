//! The `reach` command: multi-source transitive reachability over one relation.

use crate::commands::range_source::open_local_for_query;

/// `rete reach`: multi-source transitive reachability over one relation. For
/// each seed, the set of nodes it transitively reaches (or, with `--reverse`,
/// that reach it). `--parallel` fans out one rayon task per seed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn reach(
    file: &str,
    predicate: &str,
    mut seeds: Vec<String>,
    seeds_file: Option<String>,
    reverse: bool,
    parallel: bool,
    count: bool,
) -> anyhow::Result<()> {
    use std::collections::HashMap;
    let rete = open_local_for_query(file)?;
    let dict = rete.dictionary();

    if let Some(path) = seeds_file {
        for line in std::fs::read_to_string(&path)?.lines() {
            let t = line.trim();
            if !t.is_empty() && !t.starts_with('#') {
                seeds.push(t.to_string());
            }
        }
    }
    if seeds.is_empty() {
        anyhow::bail!("no seeds given (use --seed <iri> and/or --seeds-file <path>)");
    }

    // Adjacency in unified node space for the chosen direction.
    let adj: HashMap<u32, Vec<u32>> = if reverse {
        let mut m: HashMap<u32, Vec<u32>> = HashMap::new();
        for (s, o) in rete.predicate_pairs(predicate) {
            m.entry(o).or_default().push(s); // who points at o
        }
        m
    } else {
        rete_core::build_adjacency(&rete, predicate)
    };

    // Resolve seed IRIs to node ids; report unknowns rather than silently dropping.
    let mut seed_nodes = Vec::with_capacity(seeds.len());
    for s in &seeds {
        match dict.node_of_term(s) {
            Some(n) => seed_nodes.push(n),
            None => eprintln!("warning: seed not in graph, skipped: {s}"),
        }
    }

    let sets = if parallel {
        rete_core::parallel::batch_reach_parallel(&adj, &seed_nodes)
    } else {
        rete_core::batch_reach_serial(&adj, &seed_nodes)
    };

    let dir = if reverse { "reached-by" } else { "reaches" };
    for (node, set) in seed_nodes.iter().zip(sets.iter()) {
        let seed_term = dict.node_term(*node).unwrap_or_else(|| format!("#{node}"));
        if count {
            println!("{seed_term} {dir} {} node(s)", set.len());
        } else {
            println!("{seed_term} {dir} {} node(s):", set.len());
            for &n in set {
                if let Some(t) = dict.node_term(n) {
                    println!("    {t}");
                }
            }
        }
    }
    eprintln!(
        "({} seed(s), predicate {predicate}, {})",
        seed_nodes.len(),
        if parallel { "parallel" } else { "serial" }
    );
    Ok(())
}
