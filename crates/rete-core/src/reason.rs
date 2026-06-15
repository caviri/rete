//! A prototype forward-chaining OWL RL / RDFS rule reasoner.
//!
//! This is **not** a complete OWL DL/RL reasoner — it is a deliberately small,
//! transparent subset chosen to support a causal-modeling coherence check:
//! materialize the obvious RDFS/OWL entailments to a fixpoint, then flag
//! "incoherent points" (logical contradictions) such as disjoint-class
//! violations and functional-property clashes.
//!
//! Triples are canonical N-Triples token strings (`<iri>`, `"lit"`, `_:b`, …),
//! matched by exact string equality — the same representation the rest of the
//! crate uses. See `docs/reasoning.md` for the rule tables and scope.
//!
//! ## Entailment rules (materialized to fixpoint)
//! - `rdfs:subClassOf` transitivity
//! - type propagation across `rdfs:subClassOf`
//! - `rdfs:subPropertyOf` (property inheritance + transitivity)
//! - `rdfs:domain` / `rdfs:range` typing
//! - `owl:inverseOf` (both directions)
//! - `owl:SymmetricProperty`
//! - `owl:TransitiveProperty`
//!
//! ## Inconsistency rules (detected after materialization)
//! - disjoint-class membership (`owl:disjointWith`)
//! - `owl:sameAs` / `owl:differentFrom` contradiction
//! - `owl:FunctionalProperty` clash
//! - `owl:Nothing` membership

use std::collections::HashSet;

/// Version tag of this reasoner's rule set. Stamped into a baked coherence card so
/// a `coherent: true` can never be misread as a guarantee from a *different* set of
/// rules. **Bump this whenever `materialize`/`detect_inconsistencies` changes** (a
/// rule added/removed/altered), so `rete reason --verify-card` rejects a stale stamp.
pub const REASON_RULESET: &str = "owl-rl-subset/v1";

// --- Vocabulary IRIs, as canonical N-Triples tokens -------------------------

const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
const RDFS_SUBCLASS_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
const RDFS_SUBPROPERTY_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subPropertyOf>";
const RDFS_DOMAIN: &str = "<http://www.w3.org/2000/01/rdf-schema#domain>";
const RDFS_RANGE: &str = "<http://www.w3.org/2000/01/rdf-schema#range>";

const OWL_INVERSE_OF: &str = "<http://www.w3.org/2002/07/owl#inverseOf>";
const OWL_SYMMETRIC_PROPERTY: &str = "<http://www.w3.org/2002/07/owl#SymmetricProperty>";
const OWL_TRANSITIVE_PROPERTY: &str = "<http://www.w3.org/2002/07/owl#TransitiveProperty>";
const OWL_FUNCTIONAL_PROPERTY: &str = "<http://www.w3.org/2002/07/owl#FunctionalProperty>";
const OWL_DISJOINT_WITH: &str = "<http://www.w3.org/2002/07/owl#disjointWith>";
const OWL_SAME_AS: &str = "<http://www.w3.org/2002/07/owl#sameAs>";
const OWL_DIFFERENT_FROM: &str = "<http://www.w3.org/2002/07/owl#differentFrom>";
const OWL_NOTHING: &str = "<http://www.w3.org/2002/07/owl#Nothing>";

/// One detected incoherent point (a logical contradiction in the graph).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inconsistency {
    /// A short stable category, e.g. `"disjoint-classes"`.
    pub kind: &'static str,
    /// A human-readable description naming the offending terms.
    pub detail: String,
}

/// The result of reasoning over a base graph.
#[derive(Debug, Clone, Default)]
pub struct Reasoning {
    /// Newly entailed triples (those not already present in the base graph).
    pub inferred: Vec<(String, String, String)>,
    /// Detected incoherent points, computed after materialization.
    pub inconsistencies: Vec<Inconsistency>,
}

type Triple = (String, String, String);

/// Forward-chain the supported RDFS/OWL rules to a fixpoint, then scan for
/// inconsistencies over the closed graph. `inferred` excludes triples already
/// present in `base_triples`.
pub fn reason(base_triples: &[Triple]) -> Reasoning {
    // The working set is the deductive closure; `base` lets us report only the
    // *newly* entailed triples at the end.
    let base: HashSet<Triple> = base_triples.iter().cloned().collect();
    let mut all: HashSet<Triple> = base.clone();

    materialize(&mut all);

    let mut inferred: Vec<Triple> = all.iter().filter(|t| !base.contains(*t)).cloned().collect();
    inferred.sort();

    let inconsistencies = detect_inconsistencies(&all);

    Reasoning {
        inferred,
        inconsistencies,
    }
}

/// Iterate the entailment rules until no new triple is produced (fixpoint).
fn materialize(all: &mut HashSet<Triple>) {
    loop {
        // Snapshot to iterate while we collect additions; apply at round end so
        // pattern matching always sees a consistent set.
        let snapshot: Vec<Triple> = all.iter().cloned().collect();
        let mut new: Vec<Triple> = Vec::new();

        // Pre-bucket the schema axioms we need to pair with data triples.
        let subclass: Vec<(&str, &str)> = snapshot
            .iter()
            .filter(|(_, p, _)| p == RDFS_SUBCLASS_OF)
            .map(|(s, _, o)| (s.as_str(), o.as_str()))
            .collect();
        let subprop: Vec<(&str, &str)> = snapshot
            .iter()
            .filter(|(_, p, _)| p == RDFS_SUBPROPERTY_OF)
            .map(|(s, _, o)| (s.as_str(), o.as_str()))
            .collect();
        let domains: Vec<(&str, &str)> = snapshot
            .iter()
            .filter(|(_, p, _)| p == RDFS_DOMAIN)
            .map(|(s, _, o)| (s.as_str(), o.as_str()))
            .collect();
        let ranges: Vec<(&str, &str)> = snapshot
            .iter()
            .filter(|(_, p, _)| p == RDFS_RANGE)
            .map(|(s, _, o)| (s.as_str(), o.as_str()))
            .collect();
        let inverses: Vec<(&str, &str)> = snapshot
            .iter()
            .filter(|(_, p, _)| p == OWL_INVERSE_OF)
            .map(|(s, _, o)| (s.as_str(), o.as_str()))
            .collect();
        let symmetric: HashSet<&str> = snapshot
            .iter()
            .filter(|(_, p, o)| p == RDF_TYPE && o == OWL_SYMMETRIC_PROPERTY)
            .map(|(s, _, _)| s.as_str())
            .collect();
        let transitive: HashSet<&str> = snapshot
            .iter()
            .filter(|(_, p, o)| p == RDF_TYPE && o == OWL_TRANSITIVE_PROPERTY)
            .map(|(s, _, _)| s.as_str())
            .collect();

        let emit = |s: &str, p: &str, o: &str, new: &mut Vec<Triple>| {
            let t = (s.to_string(), p.to_string(), o.to_string());
            if !all.contains(&t) {
                new.push(t);
            }
        };

        for (s, p, o) in &snapshot {
            let (s, p, o) = (s.as_str(), p.as_str(), o.as_str());

            // rdfs:subClassOf transitivity: c ⊑ d . d ⊑ e ⇒ c ⊑ e
            if p == RDFS_SUBCLASS_OF {
                for (d2, e) in &subclass {
                    if *d2 == o {
                        emit(s, RDFS_SUBCLASS_OF, e, &mut new);
                    }
                }
            }

            // rdfs:subPropertyOf transitivity: p ⊑ q . q ⊑ r ⇒ p ⊑ r
            if p == RDFS_SUBPROPERTY_OF {
                for (q2, r) in &subprop {
                    if *q2 == o {
                        emit(s, RDFS_SUBPROPERTY_OF, r, &mut new);
                    }
                }
            }

            if p == RDF_TYPE {
                // type propagation: x a c . c ⊑ d ⇒ x a d
                for (c, d) in &subclass {
                    if *c == o {
                        emit(s, RDF_TYPE, d, &mut new);
                    }
                }
            } else {
                // rdfs:subPropertyOf: p ⊑ q . x p y ⇒ x q y
                for (p2, q) in &subprop {
                    if *p2 == p {
                        emit(s, q, o, &mut new);
                    }
                }
                // rdfs:domain: p domain c . x p y ⇒ x a c
                for (pr, c) in &domains {
                    if *pr == p {
                        emit(s, RDF_TYPE, c, &mut new);
                    }
                }
                // rdfs:range: p range c . x p y ⇒ y a c
                for (pr, c) in &ranges {
                    if *pr == p {
                        emit(o, RDF_TYPE, c, &mut new);
                    }
                }
                // owl:inverseOf: p inverseOf q . x p y ⇒ y q x (both directions)
                for (a, b) in &inverses {
                    if *a == p {
                        emit(o, b, s, &mut new);
                    }
                    if *b == p {
                        emit(o, a, s, &mut new);
                    }
                }
                // owl:SymmetricProperty: x p y ⇒ y p x
                if symmetric.contains(p) {
                    emit(o, p, s, &mut new);
                }
                // owl:TransitiveProperty: x p y . y p z ⇒ x p z
                if transitive.contains(p) {
                    for (s2, p2, z) in &snapshot {
                        if p2 == p && s2 == o {
                            emit(s, p, z, &mut new);
                        }
                    }
                }
            }
        }

        if new.is_empty() {
            break;
        }
        for t in new {
            all.insert(t);
        }
    }
}

/// Scan the materialized graph for incoherent points.
fn detect_inconsistencies(all: &HashSet<Triple>) -> Vec<Inconsistency> {
    let mut out: Vec<Inconsistency> = Vec::new();

    // Index helpers over the closed graph.
    let types: Vec<(&str, &str)> = all
        .iter()
        .filter(|(_, p, _)| p == RDF_TYPE)
        .map(|(s, _, o)| (s.as_str(), o.as_str()))
        .collect();

    // Disjoint pairs, recorded symmetrically so direction doesn't matter.
    let mut disjoint: HashSet<(&str, &str)> = HashSet::new();
    for (s, p, o) in all {
        if p == OWL_DISJOINT_WITH {
            disjoint.insert((s.as_str(), o.as_str()));
            disjoint.insert((o.as_str(), s.as_str()));
        }
    }

    // sameAs / differentFrom pairs (symmetric).
    let mut same_as: HashSet<(&str, &str)> = HashSet::new();
    let mut different_from: HashSet<(&str, &str)> = HashSet::new();
    for (s, p, o) in all {
        if p == OWL_SAME_AS {
            same_as.insert((s.as_str(), o.as_str()));
            same_as.insert((o.as_str(), s.as_str()));
        } else if p == OWL_DIFFERENT_FROM {
            different_from.insert((s.as_str(), o.as_str()));
            different_from.insert((o.as_str(), s.as_str()));
        }
    }

    // --- Disjoint classes: x a c . x a d . c disjointWith d ------------------
    // Group an individual's class set, then check each unordered class pair.
    let mut seen_disjoint: HashSet<(&str, &str, &str)> = HashSet::new();
    for (x, c) in &types {
        for (x2, d) in &types {
            if x != x2 || c == d {
                continue;
            }
            if disjoint.contains(&(*c, *d)) {
                // Canonicalize the (x, class-pair) so we report each clash once.
                let (lo, hi) = if c < d { (*c, *d) } else { (*d, *c) };
                if seen_disjoint.insert((*x, lo, hi)) {
                    out.push(Inconsistency {
                        kind: "disjoint-classes",
                        detail: format!(
                            "{x} is typed as both {lo} and {hi}, which are owl:disjointWith"
                        ),
                    });
                }
            }
        }
    }

    // --- sameAs / differentFrom contradiction --------------------------------
    let mut seen_same: HashSet<(&str, &str)> = HashSet::new();
    for (x, y) in &same_as {
        if different_from.contains(&(*x, *y)) {
            let (lo, hi) = if x < y { (*x, *y) } else { (*y, *x) };
            if seen_same.insert((lo, hi)) {
                out.push(Inconsistency {
                    kind: "sameas-differentfrom",
                    detail: format!("{lo} and {hi} are both owl:sameAs and owl:differentFrom"),
                });
            }
        }
    }

    // --- Functional property: p a FunctionalProperty . x p y . x p z . y≠z ----
    let functional: HashSet<&str> = all
        .iter()
        .filter(|(_, p, o)| p == RDF_TYPE && o == OWL_FUNCTIONAL_PROPERTY)
        .map(|(s, _, _)| s.as_str())
        .collect();
    if !functional.is_empty() {
        let mut seen_func: HashSet<(&str, &str, &str, &str)> = HashSet::new();
        for (s, p, o) in all {
            if !functional.contains(p.as_str()) {
                continue;
            }
            for (s2, p2, o2) in all {
                if p2 != p || s2 != s || o2 == o {
                    continue;
                }
                // Not a clash if the two values are asserted owl:sameAs.
                if same_as.contains(&(o.as_str(), o2.as_str())) {
                    continue;
                }
                let (lo, hi) = if o < o2 {
                    (o.as_str(), o2.as_str())
                } else {
                    (o2.as_str(), o.as_str())
                };
                if seen_func.insert((s.as_str(), p.as_str(), lo, hi)) {
                    out.push(Inconsistency {
                        kind: "functional-property",
                        detail: format!(
                            "functional property {p} on {s} has distinct values {lo} and {hi}"
                        ),
                    });
                }
            }
        }
    }

    // --- owl:Nothing membership ----------------------------------------------
    for (x, c) in &types {
        if *c == OWL_NOTHING {
            out.push(Inconsistency {
                kind: "owl-nothing",
                detail: format!("{x} is a member of owl:Nothing (the empty class)"),
            });
        }
    }

    out.sort_by(|a, b| (a.kind, &a.detail).cmp(&(b.kind, &b.detail)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str, p: &str, o: &str) -> Triple {
        (s.to_string(), p.to_string(), o.to_string())
    }

    /// Convenience: does the inferred set contain this triple?
    fn inferred_has(r: &Reasoning, s: &str, p: &str, o: &str) -> bool {
        r.inferred.contains(&t(s, p, o))
    }

    // --- Entailment rules ----------------------------------------------------

    #[test]
    fn subclass_transitivity() {
        let base = vec![
            t("<c>", RDFS_SUBCLASS_OF, "<d>"),
            t("<d>", RDFS_SUBCLASS_OF, "<e>"),
        ];
        let r = reason(&base);
        assert!(inferred_has(&r, "<c>", RDFS_SUBCLASS_OF, "<e>"));
    }

    #[test]
    fn type_propagation_over_subclass() {
        let base = vec![t("<x>", RDF_TYPE, "<c>"), t("<c>", RDFS_SUBCLASS_OF, "<d>")];
        let r = reason(&base);
        assert!(inferred_has(&r, "<x>", RDF_TYPE, "<d>"));
    }

    #[test]
    fn subproperty_propagation_and_transitivity() {
        let base = vec![
            t("<p>", RDFS_SUBPROPERTY_OF, "<q>"),
            t("<q>", RDFS_SUBPROPERTY_OF, "<r>"),
            t("<x>", "<p>", "<y>"),
        ];
        let r = reason(&base);
        // p ⊑ r (transitive), and x q y / x r y (inheritance).
        assert!(inferred_has(&r, "<p>", RDFS_SUBPROPERTY_OF, "<r>"));
        assert!(inferred_has(&r, "<x>", "<q>", "<y>"));
        assert!(inferred_has(&r, "<x>", "<r>", "<y>"));
    }

    #[test]
    fn domain_typing() {
        let base = vec![t("<p>", RDFS_DOMAIN, "<C>"), t("<x>", "<p>", "<y>")];
        let r = reason(&base);
        assert!(inferred_has(&r, "<x>", RDF_TYPE, "<C>"));
    }

    #[test]
    fn range_typing() {
        let base = vec![t("<p>", RDFS_RANGE, "<C>"), t("<x>", "<p>", "<y>")];
        let r = reason(&base);
        assert!(inferred_has(&r, "<y>", RDF_TYPE, "<C>"));
    }

    #[test]
    fn inverse_of_both_directions() {
        let base = vec![
            t("<p>", OWL_INVERSE_OF, "<q>"),
            t("<x>", "<p>", "<y>"),
            t("<a>", "<q>", "<b>"),
        ];
        let r = reason(&base);
        assert!(inferred_has(&r, "<y>", "<q>", "<x>"));
        assert!(inferred_has(&r, "<b>", "<p>", "<a>"));
    }

    #[test]
    fn symmetric_property() {
        let base = vec![
            t("<p>", RDF_TYPE, OWL_SYMMETRIC_PROPERTY),
            t("<x>", "<p>", "<y>"),
        ];
        let r = reason(&base);
        assert!(inferred_has(&r, "<y>", "<p>", "<x>"));
    }

    #[test]
    fn transitive_property() {
        let base = vec![
            t("<p>", RDF_TYPE, OWL_TRANSITIVE_PROPERTY),
            t("<x>", "<p>", "<y>"),
            t("<y>", "<p>", "<z>"),
        ];
        let r = reason(&base);
        assert!(inferred_has(&r, "<x>", "<p>", "<z>"));
    }

    // --- Inconsistency rules -------------------------------------------------

    #[test]
    fn disjoint_classes_detected() {
        let base = vec![
            t("<C>", OWL_DISJOINT_WITH, "<D>"),
            t("<x>", RDF_TYPE, "<C>"),
            t("<x>", RDF_TYPE, "<D>"),
        ];
        let r = reason(&base);
        assert!(r
            .inconsistencies
            .iter()
            .any(|i| i.kind == "disjoint-classes"));
    }

    #[test]
    fn disjoint_classes_via_subclass_propagation() {
        // The clash only surfaces after type propagation: x is a C, C ⊑ D,
        // and D is disjoint with E, x is an E.
        let base = vec![
            t("<C>", RDFS_SUBCLASS_OF, "<D>"),
            t("<D>", OWL_DISJOINT_WITH, "<E>"),
            t("<x>", RDF_TYPE, "<C>"),
            t("<x>", RDF_TYPE, "<E>"),
        ];
        let r = reason(&base);
        assert!(
            r.inconsistencies
                .iter()
                .any(|i| i.kind == "disjoint-classes"),
            "expected a disjoint-classes clash exposed by subClassOf propagation"
        );
    }

    #[test]
    fn sameas_differentfrom_detected() {
        let base = vec![
            t("<x>", OWL_SAME_AS, "<y>"),
            t("<y>", OWL_DIFFERENT_FROM, "<x>"),
        ];
        let r = reason(&base);
        assert!(r
            .inconsistencies
            .iter()
            .any(|i| i.kind == "sameas-differentfrom"));
    }

    #[test]
    fn functional_property_clash_detected() {
        let base = vec![
            t("<p>", RDF_TYPE, OWL_FUNCTIONAL_PROPERTY),
            t("<x>", "<p>", "<y>"),
            t("<x>", "<p>", "<z>"),
        ];
        let r = reason(&base);
        assert!(r
            .inconsistencies
            .iter()
            .any(|i| i.kind == "functional-property"));
    }

    #[test]
    fn functional_property_sameas_is_coherent() {
        // Two values that are owl:sameAs are NOT a functional clash.
        let base = vec![
            t("<p>", RDF_TYPE, OWL_FUNCTIONAL_PROPERTY),
            t("<x>", "<p>", "<y>"),
            t("<x>", "<p>", "<z>"),
            t("<y>", OWL_SAME_AS, "<z>"),
        ];
        let r = reason(&base);
        assert!(!r
            .inconsistencies
            .iter()
            .any(|i| i.kind == "functional-property"));
    }

    #[test]
    fn owl_nothing_detected() {
        let base = vec![t("<x>", RDF_TYPE, OWL_NOTHING)];
        let r = reason(&base);
        assert!(r.inconsistencies.iter().any(|i| i.kind == "owl-nothing"));
    }

    #[test]
    fn coherent_graph_has_no_inconsistencies() {
        let base = vec![
            t("<C>", RDFS_SUBCLASS_OF, "<D>"),
            t("<x>", RDF_TYPE, "<C>"),
            t("<p>", RDF_TYPE, OWL_TRANSITIVE_PROPERTY),
            t("<x>", "<p>", "<y>"),
            t("<y>", "<p>", "<z>"),
        ];
        let r = reason(&base);
        assert!(
            r.inconsistencies.is_empty(),
            "expected coherent graph, got {:?}",
            r.inconsistencies
        );
        // Sanity: it still entailed something.
        assert!(!r.inferred.is_empty());
    }

    #[test]
    fn inferred_excludes_base_triples() {
        let base = vec![t("<x>", RDF_TYPE, "<c>"), t("<c>", RDFS_SUBCLASS_OF, "<d>")];
        let r = reason(&base);
        // The base triples must not appear in `inferred`.
        for b in &base {
            assert!(!r.inferred.contains(b));
        }
        assert!(inferred_has(&r, "<x>", RDF_TYPE, "<d>"));
    }

    #[test]
    fn end_to_end_small_ontology() {
        // A tiny causal ontology + data: Cause ⊑ Event, :causes is transitive,
        // Healthy disjointWith Sick, and a patient typed as both.
        let cause = "<http://ex/Cause>";
        let event = "<http://ex/Event>";
        let causes = "<http://ex/causes>";
        let healthy = "<http://ex/Healthy>";
        let sick = "<http://ex/Sick>";
        let base = vec![
            t(cause, RDFS_SUBCLASS_OF, event),
            t(causes, RDF_TYPE, OWL_TRANSITIVE_PROPERTY),
            t(healthy, OWL_DISJOINT_WITH, sick),
            t("<http://ex/a>", RDF_TYPE, cause),
            t("<http://ex/a>", causes, "<http://ex/b>"),
            t("<http://ex/b>", causes, "<http://ex/c>"),
            t("<http://ex/p>", RDF_TYPE, healthy),
            t("<http://ex/p>", RDF_TYPE, sick),
        ];
        let r = reason(&base);
        // Entailments: a is an Event; a causes c (transitivity).
        assert!(inferred_has(&r, "<http://ex/a>", RDF_TYPE, event));
        assert!(inferred_has(&r, "<http://ex/a>", causes, "<http://ex/c>"));
        // Incoherent point: p is both Healthy and Sick.
        assert!(r
            .inconsistencies
            .iter()
            .any(|i| i.kind == "disjoint-classes" && i.detail.contains("http://ex/p")));
    }
}
