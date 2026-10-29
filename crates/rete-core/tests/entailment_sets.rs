//! OWL 2 QL entailment (`--entail`) answers each entailed triple ONCE.
//!
//! The rewriting answers `?x a C` by the asserted patterns that entail it —
//! `?x a D` for every `D ⊑* C`, `?x p ?_` for every `p` with a domain `⊑* C`,
//! `?_ q ?x` for every `q` with a range `⊑* C` — and a node matching several of
//! them (typed both `fabio:Expression` and `fabio:JournalArticle`, as every
//! OpenCitations work is) used to come back once per match: `COUNT(?s)` said
//! 508,044 where 258,268 works exist. Role atoms had the same defect through
//! subproperties and inverses. Every answer here is written out by hand and
//! checked on a six-permutation file and on a three-permutation one (no PSO, so
//! a domain source cannot stream in subject order and takes the fallback).

use rete_core::ingest::assemble_dataset_with_perms;
use rete_core::{
    eval_query_reasoned, eval_sparql, eval_sparql_reasoned, PermSet, PyramidAlgo, QueryOutput, Rete,
};

const P: &str = "PREFIX ex: <http://ex/> ";

fn quads() -> Vec<(String, String, String, Option<String>)> {
    let e = |n: &str| format!("<http://ex/{n}>");
    let ty = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>".to_string();
    let sub = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>".to_string();
    let subp = "<http://www.w3.org/2000/01/rdf-schema#subPropertyOf>".to_string();
    let dom = "<http://www.w3.org/2000/01/rdf-schema#domain>".to_string();
    let rng = "<http://www.w3.org/2000/01/rdf-schema#range>".to_string();
    let inv = "<http://www.w3.org/2002/07/owl#inverseOf>".to_string();
    let t = |s: &str, p: &String, o: &str| (e(s), p.clone(), e(o), None);
    let r = |s: &str, p: &str, o: &str| (e(s), e(p), e(o), None);
    vec![
        // TBox.
        t("JournalArticle", &sub, "Expression"),
        t("Book", &sub, "Expression"),
        t("Chapter", &sub, "Book"),
        t("writtenBy", &dom, "Expression"),
        t("writtenBy", &rng, "Person"),
        // An axiom about a property the data never uses, on a class nothing
        // else mentions: it must not touch any other atom.
        t("hasCharacterisation", &dom, "Citation"),
        t("hasFather", &subp, "hasParent"),
        t("hasChild", &inv, "hasParent"),
        // ABox. w1 entails `a Expression` three ways, w3 three ways.
        t("w1", &ty, "Expression"),
        t("w1", &ty, "JournalArticle"),
        t("w2", &ty, "JournalArticle"),
        t("w3", &ty, "Expression"),
        t("w3", &ty, "Book"),
        t("w3", &ty, "Chapter"),
        t("w4", &ty, "Book"),
        r("w1", "writtenBy", "ann"),
        r("w5", "writtenBy", "ann"),
        r("w5", "writtenBy", "bob"),
        t("ann", &ty, "Person"),
        t("ann", &ty, "Agent"),
        // (a, b) is stated three ways: hasFather, hasParent, and hasChild back.
        r("a", "hasFather", "b"),
        r("a", "hasParent", "b"),
        r("b", "hasChild", "a"),
        r("c", "hasFather", "d"),
        r("e", "hasChild", "f"),
    ]
}

fn files() -> Vec<(&'static str, Rete)> {
    [("6 perms", PermSet::ALL), ("3 perms", PermSet::CORE)]
        .into_iter()
        .map(|(name, perms)| {
            let (bytes, _) = assemble_dataset_with_perms(
                quads(),
                false,
                false,
                None,
                PyramidAlgo::Louvain,
                perms,
                |_, _| Vec::new(),
            );
            (name, Rete::open(&bytes).unwrap())
        })
        .collect()
}

fn ex(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| format!("<http://ex/{n}>")).collect()
}

/// The `var` column of a reasoned SELECT, sorted but NOT deduplicated.
fn col(rete: &Rete, q: &str, var: &str) -> Vec<String> {
    let (_, sols) = eval_sparql_reasoned(rete, &format!("{P}{q}")).unwrap();
    let mut v: Vec<String> = sols.iter().filter_map(|b| b.get(var).cloned()).collect();
    v.sort();
    v
}

fn count(rete: &Rete, q: &str) -> String {
    let (_, sols) = eval_sparql_reasoned(rete, &format!("{P}{q}")).unwrap();
    let n = sols[0]["n"].clone();
    n.split('"').nth(1).unwrap().to_string()
}

#[test]
fn a_class_atom_answers_each_member_once() {
    for (name, rete) in files() {
        // Typed directly, through two levels of subclass, and through the
        // domain of writtenBy — each once.
        assert_eq!(
            col(&rete, "SELECT ?s WHERE { ?s a ex:Expression }", "s"),
            ex(&["w1", "w2", "w3", "w4", "w5"]),
            "{name}"
        );
        assert_eq!(
            count(
                &rete,
                "SELECT (COUNT(?s) AS ?n) WHERE { ?s a ex:Expression }"
            ),
            "5",
            "{name}"
        );
        assert_eq!(
            count(
                &rete,
                "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s a ex:Expression }"
            ),
            "5",
            "{name}"
        );
        assert_eq!(
            col(&rete, "SELECT ?s WHERE { ?s a ex:Book }", "s"),
            ex(&["w3", "w4"]),
            "{name}"
        );
        // Range: writtenBy's objects are Persons; ann is also typed so.
        assert_eq!(
            col(&rete, "SELECT ?s WHERE { ?s a ex:Person }", "s"),
            ex(&["ann", "bob"]),
            "{name}"
        );
        // Joined with another pattern, with and without a LIMIT (the
        // semi-join and the merged enumeration give the same rows).
        let joined = "SELECT ?s ?p WHERE { ?s ex:writtenBy ?p . ?s a ex:Expression }";
        let all = col(&rete, joined, "s");
        assert_eq!(all, ex(&["w1", "w5", "w5"]), "{name}");
        assert_eq!(
            col(&rete, &format!("{joined} LIMIT 10"), "s"),
            all,
            "{name} LIMIT"
        );
        // OPTIONAL over an entailed atom keeps every left row once.
        assert_eq!(
            col(
                &rete,
                "SELECT ?s WHERE { ?s ex:writtenBy ?p OPTIONAL { ?s a ex:Book } }",
                "s"
            ),
            ex(&["w1", "w5", "w5"]),
            "{name} OPTIONAL"
        );
    }
}

#[test]
fn a_constant_class_atom_is_a_yes_or_no() {
    for (name, rete) in files() {
        let ask = |q: &str| match eval_query_reasoned(&rete, &format!("{P}{q}")).unwrap() {
            QueryOutput::Ask(b) => b,
            other => panic!("{other:?}"),
        };
        assert!(ask("ASK { ex:w3 a ex:Expression }"), "{name}");
        assert!(ask("ASK { ex:w5 a ex:Expression }"), "{name}");
        assert!(!ask("ASK { ex:ann a ex:Expression }"), "{name}");
        // One row, not one per way w3 is an Expression.
        assert_eq!(
            col(
                &rete,
                "SELECT ?t WHERE { ex:w3 a ex:Expression . ex:w3 a ?t }",
                "t"
            ),
            ex(&["Book", "Chapter", "Expression"]),
            "{name}"
        );
    }
}

#[test]
fn a_role_atom_answers_each_pair_once() {
    for (name, rete) in files() {
        let (_, sols) = eval_sparql_reasoned(
            &rete,
            &format!("{P}SELECT ?x ?y WHERE {{ ?x ex:hasParent ?y }}"),
        )
        .unwrap();
        let mut pairs: Vec<String> = sols
            .iter()
            .map(|b| format!("{} {}", b["x"], b["y"]))
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            vec![
                "<http://ex/a> <http://ex/b>",
                "<http://ex/c> <http://ex/d>",
                "<http://ex/f> <http://ex/e>",
            ],
            "{name}"
        );
        // A bound end.
        assert_eq!(
            col(&rete, "SELECT ?y WHERE { ex:a ex:hasParent ?y }", "y"),
            ex(&["b"]),
            "{name}"
        );
        assert_eq!(
            col(&rete, "SELECT ?x WHERE { ?x ex:hasParent ex:b }", "x"),
            ex(&["a"]),
            "{name}"
        );
    }
}

#[test]
fn without_reasoning_nothing_changes() {
    for (name, rete) in files() {
        let (_, sols) = eval_sparql(
            &rete,
            &format!("{P}SELECT ?s WHERE {{ ?s a ex:Expression }}"),
        )
        .unwrap();
        assert_eq!(sols.len(), 2, "{name}");
        // An atom reasoning cannot extend is answered exactly as written.
        assert_eq!(
            col(&rete, "SELECT ?s WHERE { ?s a ex:Agent }", "s"),
            ex(&["ann"]),
            "{name}"
        );
        assert!(col(&rete, "SELECT ?s WHERE { ?s a ex:Citation }", "s").is_empty());
    }
}

/// A class with many subclasses reads its whole `rdf:type` extent as one scan
/// when the subclasses cover most of it (the per-class cursors would each
/// fault a tile before the first row); the answers are the same either way,
/// and the three-permutation file (no PSO to scan in subject order) keeps the
/// per-class merge.
#[test]
fn a_deep_hierarchy_answers_each_member_once() {
    let e = |n: String| format!("<http://ex/{n}>");
    let ty = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>".to_string();
    let sub = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>".to_string();
    let mut q: Vec<(String, String, String, Option<String>)> = Vec::new();
    for c in 0..12 {
        q.push((e(format!("C{c}")), sub.clone(), e("Animal".into()), None));
    }
    for i in 0..60 {
        let x = e(format!("x{i}"));
        q.push((x.clone(), ty.clone(), e(format!("C{}", i % 12)), None));
        q.push((x.clone(), ty.clone(), e(format!("C{}", (i * 7) % 12)), None));
        if i % 3 == 0 {
            q.push((x, ty.clone(), e("Animal".into()), None));
        }
    }
    q.push((e("plant".into()), ty.clone(), e("Plant".into()), None));
    for (name, perms) in [("6 perms", PermSet::ALL), ("3 perms", PermSet::CORE)] {
        let (bytes, _) = assemble_dataset_with_perms(
            q.clone(),
            false,
            false,
            None,
            PyramidAlgo::Louvain,
            perms,
            |_, _| Vec::new(),
        );
        let rete = Rete::open(&bytes).unwrap();
        let got = col(&rete, "SELECT ?x WHERE { ?x a ex:Animal }", "x");
        let mut want: Vec<String> = (0..60).map(|i| e(format!("x{i}"))).collect();
        want.sort();
        assert_eq!(got, want, "{name}");
        assert_eq!(
            col(&rete, "SELECT ?x WHERE { ?x a ex:Animal } LIMIT 7", "x").len(),
            7,
            "{name}"
        );
        // The semi-join: a bound subject checks its own types once.
        assert_eq!(
            col(
                &rete,
                "SELECT ?x WHERE { VALUES ?x { ex:x5 ex:plant ex:x59 } ?x a ex:Animal } LIMIT 5",
                "x"
            ),
            vec![e("x59".into()), e("x5".into())], // sorted as strings
            "{name}"
        );
    }
}
