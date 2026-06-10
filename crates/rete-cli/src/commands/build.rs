//! The ingest group: parse RDF inputs (N-Triples / N-Quads / Turtle) and either
//! `build` them into a `.rete` dataset or `validate` that they parse. Parsing
//! and assembly live in `rete_core::ingest` (shared with the wasm bindings);
//! this module adds the CLI-only concerns: file/stdin IO, format detection by
//! extension, `--materialize`, and the Dataset Card flags.

use crate::commands::card::{self, CardArgs};
use rete_core::ingest;

/// Read an input source: a file path, or `-` for stdin.
fn read_input(path: &str) -> anyhow::Result<String> {
    if path == "-" {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        Ok(std::fs::read_to_string(path)?)
    }
}

/// The parser to use for an input: explicit `--format` wins, else by extension,
/// else (no extension / stdin) N-Triples.
fn input_format(path: &str, override_fmt: Option<&str>) -> &'static str {
    if let Some(f) = override_fmt {
        return match f {
            "nq" => "nq",
            "ttl" => "ttl",
            _ => "nt",
        };
    }
    let p = path.to_ascii_lowercase();
    if p.ends_with(".nq") || p.ends_with(".nquads") {
        "nq"
    } else if p.ends_with(".ttl") || p.ends_with(".turtle") {
        "ttl"
    } else {
        "nt"
    }
}

/// Parse one or more RDF inputs into quads (triples → default graph). Shared by
/// `build` and `validate`. Returns a parse error (which input, what went wrong)
/// if any input is malformed.
fn parse_inputs(inputs: &[String], format: Option<&str>) -> anyhow::Result<Vec<ingest::RawQuad>> {
    let mut quads: Vec<ingest::RawQuad> = Vec::new();
    for input in inputs {
        let text = read_input(input)?;
        let parsed = ingest::parse_statements(&text, input_format(input, format))
            .map_err(|e| anyhow::anyhow!("{input}: {e}"))?;
        quads.extend(parsed);
    }
    Ok(quads)
}

/// Parse inputs without building — report triple/quad and named-graph counts, or
/// fail with a clear parse error. The way to check an RDF file (N-Triples /
/// N-Quads / Turtle) is well-formed before ingesting it.
pub(crate) fn validate(inputs: &[String], format: Option<&str>) -> anyhow::Result<()> {
    let quads = parse_inputs(inputs, format)?;
    let named: std::collections::BTreeSet<&String> =
        quads.iter().filter_map(|(_, _, _, g)| g.as_ref()).collect();
    let in_default = quads.iter().filter(|(_, _, _, g)| g.is_none()).count();
    println!(
        "valid: {} statement(s) — {} in the default graph, {} named graph(s)",
        quads.len(),
        in_default,
        named.len()
    );
    Ok(())
}

/// Build a `.rete` from one or more inputs, merged under one shared dictionary.
/// N-Triples/Turtle contribute to the default graph; N-Quads may carry named
/// graphs. The output uses the dataset layout iff any named graph appears (which
/// is byte-identical to the plain triple file when none do).
pub(crate) fn build(
    inputs: &[String],
    output: &str,
    format: Option<&str>,
    materialize: bool,
    card_args: CardArgs,
) -> anyhow::Result<()> {
    // 1. Parse every input into quads (triples → default graph, `None`).
    let mut quads = parse_inputs(inputs, format)?;

    // 1b. Optionally materialize RDFS/OWL-RL entailments over the default graph
    // and fold the inferred triples in (deduped later by the index builder), so
    // they ship in the file and need no query-time reasoning. A logically
    // incoherent graph aborts the build rather than baking in a contradiction.
    if materialize {
        let base: Vec<(String, String, String)> = quads
            .iter()
            .filter(|(_, _, _, g)| g.is_none())
            .map(|(s, p, o, _)| (s.clone(), p.clone(), o.clone()))
            .collect();
        let reasoning = rete_core::reason(&base);
        if !reasoning.inconsistencies.is_empty() {
            for inc in &reasoning.inconsistencies {
                eprintln!("  incoherent [{}] {}", inc.kind, inc.detail);
            }
            anyhow::bail!(
                "{} inconsistency(ies) — refusing to materialize an incoherent graph",
                reasoning.inconsistencies.len()
            );
        }
        let inferred = reasoning.inferred.len();
        quads.extend(
            reasoning
                .inferred
                .into_iter()
                .map(|(s, p, o)| (s, p, o, None)),
        );
        eprintln!("materialized {inferred} inferred triple(s) into the default graph");
    }

    // 2. Assemble dictionary + indexes + pyramid into the file image, optionally
    // deriving + embedding a Dataset Card (data-catalog metadata) from the final
    // counts. Without a card flag the metadata payload is empty, which is
    // byte-identical to a metadata-free build.
    let curated = if card_args.requested() {
        Some(card::load_curated(&card_args)?)
    } else {
        None
    };
    let (bytes, stats) = ingest::assemble_dataset_with(&quads, |stats| match curated {
        Some(curated) => {
            let dataset_card = card::derive_card(
                &quads,
                stats.terms as u64,
                stats.named_graphs as u64,
                curated,
            );
            let blob = dataset_card.to_json_bytes();
            eprintln!("embedded dataset card ({} bytes of metadata)", blob.len());
            blob
        }
        None => Vec::new(),
    });
    std::fs::write(output, &bytes)?;

    if stats.named_graphs > 0 {
        println!(
            "wrote {output}: {} quads ({} default + {} named graph(s)), {} terms, {} bytes",
            stats.statements,
            stats.default_triples,
            stats.named_graphs,
            stats.terms,
            bytes.len()
        );
    } else {
        println!(
            "wrote {output}: {} triples, {} terms, {} pyramid level(s), {} bytes",
            stats.default_triples,
            stats.terms,
            stats.pyramid_levels,
            bytes.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rete_core::Rete;

    /// `build --materialize` runs the reasoner and bakes RDFS entailments into the
    /// file: `x a C` + `C subClassOf D` yields a queryable `x a D`.
    #[test]
    fn build_materialize_bakes_in_entailments() {
        const TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
        const SUBCLASS: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
        let nt = format!("<http://x> {TYPE} <http://C> .\n<http://C> {SUBCLASS} <http://D> .\n");
        let pid = std::process::id();
        let inp = std::env::temp_dir().join(format!("rete_mat_in_{pid}.nt"));
        let out = std::env::temp_dir().join(format!("rete_mat_out_{pid}.rete"));
        std::fs::write(&inp, nt).unwrap();

        build(
            &[inp.to_str().unwrap().to_string()],
            out.to_str().unwrap(),
            None,
            true,
            CardArgs::default(),
        )
        .unwrap();

        let bytes = std::fs::read(&out).unwrap();
        let rete = Rete::open(&bytes).unwrap();
        let inferred = rete.query(Some("<http://x>"), Some(TYPE), Some("<http://D>"));
        assert_eq!(inferred.len(), 1, "x a D should be materialized");

        // Without --materialize the entailed triple is absent.
        build(
            &[inp.to_str().unwrap().to_string()],
            out.to_str().unwrap(),
            None,
            false,
            CardArgs::default(),
        )
        .unwrap();
        let plain = Rete::open(&std::fs::read(&out).unwrap()).unwrap();
        assert!(plain
            .query(Some("<http://x>"), Some(TYPE), Some("<http://D>"))
            .is_empty());

        std::fs::remove_file(&inp).ok();
        std::fs::remove_file(&out).ok();
    }
}
