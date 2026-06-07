//! The `reason` command: run the prototype OWL RL / RDFS reasoner, report
//! inferred triples + inconsistencies, and (with `--materialize`) print the
//! base + inferred graph. Exits non-zero on any inconsistency (a CI coherence
//! check).

use rete_core::Rete;

use crate::commands::export::export_turtle;

pub(crate) fn reason_cmd(file: &str, materialize: bool, format: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    let base = rete.dump(None);

    let result = rete_core::reason(&base);

    println!("inferred {} new triple(s)", result.inferred.len());
    if result.inconsistencies.is_empty() {
        println!("coherent: no inconsistencies found");
    } else {
        println!("{} inconsistency(ies) found:", result.inconsistencies.len());
        for inc in &result.inconsistencies {
            println!("  [{}] {}", inc.kind, inc.detail);
        }
    }

    if materialize {
        let mut all = base.clone();
        all.extend(result.inferred.iter().cloned());
        match format {
            "ttl" => print!("{}", export_turtle(&all)),
            _ => {
                for (s, p, o) in &all {
                    println!("{s} {p} {o} .");
                }
            }
        }
    }

    if !result.inconsistencies.is_empty() {
        anyhow::bail!(
            "{} inconsistency(ies) — graph is incoherent",
            result.inconsistencies.len()
        );
    }
    Ok(())
}
