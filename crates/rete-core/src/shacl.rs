//! SHACL Core validation over canonical Rete term triples.
//!
//! This module targets the stable W3C SHACL Core recommendation. It deliberately
//! stays independent of the `.rete` byte layout: callers provide a [`DataGraph`]
//! made from an opened [`crate::Rete`] or from raw triples, and a [`ShaclShapes`]
//! graph parsed from Turtle.

use std::collections::{BTreeSet, HashSet};

use thiserror::Error;

use crate::{Rete, TermTriple};

const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
const RDF_FIRST: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#first>";
const RDF_REST: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#rest>";
const RDF_NIL: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#nil>";
const RDFS_CLASS: &str = "<http://www.w3.org/2000/01/rdf-schema#Class>";
const RDFS_SUBCLASS_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
const OWL_CLASS: &str = "<http://www.w3.org/2002/07/owl#Class>";
const XSD_STRING: &str = "<http://www.w3.org/2001/XMLSchema#string>";
const RDF_LANG_STRING: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#langString>";

const SH: &str = "http://www.w3.org/ns/shacl#";

macro_rules! sh {
    ($local:literal) => {
        concat!("<http://www.w3.org/ns/shacl#", $local, ">")
    };
}

type Triple = (String, String, String);

#[derive(Debug, Error)]
pub enum ShaclError {
    #[error("failed to parse SHACL shapes Turtle: {0}")]
    Parse(String),
    #[error("malformed RDF list at {0}")]
    MalformedList(String),
}

/// A read-only view of the data graph for SHACL validation. The validator only
/// ever asks **targeted** questions — a focus node's values, the subjects of a
/// predicate, the instances of a class — so this surface is small enough to back
/// two ways: an in-memory triple set ([`DataGraph`], eager) or a `.rete` file's
/// index directly ([`ReteGraph`]), which routes each lookup as a range read so a
/// *remote* validation faults only the tiles holding the shapes' targets, not the
/// whole graph. The class/instance helpers are derived from the primitives, so a
/// backend only implements the six lookups.
pub trait GraphView {
    /// Objects of `(subject, predicate, ?)`.
    fn objects(&self, subject: &str, predicate: &str) -> Vec<String>;
    /// Subjects of `(?, predicate, object)`.
    fn subjects_with(&self, predicate: &str, object: &str) -> Vec<String>;
    /// Distinct subjects of `(?, predicate, ?)`.
    fn subjects_of(&self, predicate: &str) -> Vec<String>;
    /// Distinct objects of `(?, predicate, ?)`.
    fn objects_of(&self, predicate: &str) -> Vec<String>;
    /// Distinct predicates of `(subject, ?, ?)`.
    fn predicates_for_subject(&self, subject: &str) -> Vec<String>;
    /// Every node (subject or object). The one inherently **non-targeted** lookup
    /// — a remote validation that reaches it reads the whole graph (only a general
    /// inverse path or a target-less shape does).
    fn all_nodes(&self) -> Vec<String>;

    /// Is `child` a reflexive/transitive `rdfs:subClassOf` of `parent`?
    fn is_subclass_of(&self, child: &str, parent: &str) -> bool {
        if child == parent {
            return true;
        }
        let mut seen = HashSet::new();
        let mut stack = vec![child.to_string()];
        while let Some(c) = stack.pop() {
            if !seen.insert(c.clone()) {
                continue;
            }
            for sup in self.objects(&c, RDFS_SUBCLASS_OF) {
                if sup == parent {
                    return true;
                }
                stack.push(sup);
            }
        }
        false
    }

    /// The (transitive) subclasses of `parent`.
    fn subclasses_of(&self, parent: &str) -> BTreeSet<String> {
        self.subjects_of(RDFS_SUBCLASS_OF)
            .into_iter()
            .filter(|s| self.is_subclass_of(s, parent))
            .collect()
    }

    /// Instances of `class` (direct, or via a subclass).
    fn instances_of(&self, class: &str) -> Vec<String> {
        let mut classes = self.subclasses_of(class);
        classes.insert(class.to_string());
        let mut out = Vec::new();
        for c in &classes {
            out.extend(self.subjects_with(RDF_TYPE, c));
        }
        unique(out)
    }

    /// Is `node` an instance of `class` (direct, or via a subclass)?
    fn is_instance_of(&self, node: &str, class: &str) -> bool {
        self.objects(node, RDF_TYPE)
            .iter()
            .any(|c| self.is_subclass_of(c, class))
    }
}

/// A validation data graph held fully in memory as a sorted triple vector. Backs
/// the shapes graph and the **eager** data path; the lazy data path uses
/// [`ReteGraph`].
#[derive(Debug, Clone, Default)]
pub struct DataGraph {
    triples: Vec<Triple>,
}

impl DataGraph {
    pub fn from_triples(triples: Vec<TermTriple>) -> Self {
        let mut triples = triples;
        triples.sort();
        triples.dedup();
        Self { triples }
    }

    pub fn from_rete(rete: &Rete, graph: Option<&str>) -> Self {
        Self::from_triples(rete.dump(graph))
    }

    fn has(&self, s: &str, p: &str, o: &str) -> bool {
        self.triples
            .iter()
            .any(|(ts, tp, to)| ts == s && tp == p && to == o)
    }
}

impl GraphView for DataGraph {
    fn objects(&self, subject: &str, predicate: &str) -> Vec<String> {
        self.triples
            .iter()
            .filter(|(s, p, _)| s == subject && p == predicate)
            .map(|(_, _, o)| o.clone())
            .collect()
    }

    fn subjects_with(&self, predicate: &str, object: &str) -> Vec<String> {
        unique(
            self.triples
                .iter()
                .filter(|(_, p, o)| p == predicate && o == object)
                .map(|(s, _, _)| s.clone())
                .collect(),
        )
    }

    fn subjects_of(&self, predicate: &str) -> Vec<String> {
        unique(
            self.triples
                .iter()
                .filter(|(_, p, _)| p == predicate)
                .map(|(s, _, _)| s.clone())
                .collect(),
        )
    }

    fn objects_of(&self, predicate: &str) -> Vec<String> {
        unique(
            self.triples
                .iter()
                .filter(|(_, p, _)| p == predicate)
                .map(|(_, _, o)| o.clone())
                .collect(),
        )
    }

    fn predicates_for_subject(&self, subject: &str) -> Vec<String> {
        unique(
            self.triples
                .iter()
                .filter(|(s, _, _)| s == subject)
                .map(|(_, p, _)| p.clone())
                .collect(),
        )
    }

    fn all_nodes(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (s, _, o) in &self.triples {
            out.push(s.clone());
            out.push(o.clone());
        }
        unique(out)
    }
}

/// A SHACL data-graph view backed directly by a `.rete` file's index: every
/// lookup is a routed pattern query, so over a lazy
/// ([`Rete::open_ranged_lazy`](crate::Rete::open_ranged_lazy)) open a validation
/// faults only the tiles holding the shapes' target nodes — not the whole graph.
/// Views the **default** graph (named-graph validation uses the eager
/// [`DataGraph`]).
pub struct ReteGraph<'a> {
    rete: &'a Rete,
}

impl<'a> ReteGraph<'a> {
    pub fn new(rete: &'a Rete) -> Self {
        Self { rete }
    }
}

impl GraphView for ReteGraph<'_> {
    fn objects(&self, subject: &str, predicate: &str) -> Vec<String> {
        self.rete
            .query(Some(subject), Some(predicate), None)
            .into_iter()
            .map(|(_, _, o)| o)
            .collect()
    }

    fn subjects_with(&self, predicate: &str, object: &str) -> Vec<String> {
        self.rete
            .query(None, Some(predicate), Some(object))
            .into_iter()
            .map(|(s, _, _)| s)
            .collect()
    }

    fn subjects_of(&self, predicate: &str) -> Vec<String> {
        unique(
            self.rete
                .query(None, Some(predicate), None)
                .into_iter()
                .map(|(s, _, _)| s)
                .collect(),
        )
    }

    fn objects_of(&self, predicate: &str) -> Vec<String> {
        unique(
            self.rete
                .query(None, Some(predicate), None)
                .into_iter()
                .map(|(_, _, o)| o)
                .collect(),
        )
    }

    fn predicates_for_subject(&self, subject: &str) -> Vec<String> {
        unique(
            self.rete
                .query(Some(subject), None, None)
                .into_iter()
                .map(|(_, p, _)| p)
                .collect(),
        )
    }

    fn all_nodes(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (s, _, o) in self.rete.query(None, None, None) {
            out.push(s);
            out.push(o);
        }
        unique(out)
    }
}

/// Parsed SHACL shapes graph.
#[derive(Debug, Clone)]
pub struct ShaclShapes {
    graph: DataGraph,
}

impl ShaclShapes {
    pub fn parse_turtle(text: &str) -> Result<Self, ShaclError> {
        let mut triples = Vec::new();
        for r in oxttl::TurtleParser::new().for_reader(text.as_bytes()) {
            let t = r.map_err(|e| ShaclError::Parse(e.to_string()))?;
            triples.push((
                t.subject.to_string(),
                t.predicate.to_string(),
                t.object.to_string(),
            ));
        }
        Ok(Self {
            graph: DataGraph::from_triples(triples),
        })
    }

    fn objects(&self, subject: &str, predicate: &str) -> Vec<String> {
        self.graph.objects(subject, predicate)
    }

    fn subjects(&self, predicate: &str, object: &str) -> Vec<String> {
        unique(
            self.graph
                .triples
                .iter()
                .filter(|(_, p, o)| p == predicate && o == object)
                .map(|(s, _, _)| s.clone())
                .collect(),
        )
    }

    fn has(&self, s: &str, p: &str, o: &str) -> bool {
        self.graph.has(s, p, o)
    }

    fn list(&self, head: &str) -> Result<Vec<String>, ShaclError> {
        if head == RDF_NIL {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut cur = head.to_string();
        let mut seen = HashSet::new();
        loop {
            if cur == RDF_NIL {
                break;
            }
            if !seen.insert(cur.clone()) {
                return Err(ShaclError::MalformedList(head.to_string()));
            }
            let first = self.objects(&cur, RDF_FIRST);
            let rest = self.objects(&cur, RDF_REST);
            if first.len() != 1 || rest.len() != 1 {
                return Err(ShaclError::MalformedList(head.to_string()));
            }
            out.push(first[0].clone());
            cur = rest[0].clone();
        }
        Ok(out)
    }

    fn target_shapes(&self) -> Vec<String> {
        let mut ids = Vec::new();
        for (s, p, o) in &self.graph.triples {
            if matches!(
                p.as_str(),
                sh!("targetNode")
                    | sh!("targetClass")
                    | sh!("targetSubjectsOf")
                    | sh!("targetObjectsOf")
            ) || (p == RDF_TYPE
                && matches!(
                    o.as_str(),
                    sh!("NodeShape") | sh!("PropertyShape") | RDFS_CLASS | OWL_CLASS
                ))
            {
                ids.push(s.clone());
            }
        }
        unique(ids)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Violation,
    Other(String),
}

impl Severity {
    fn from_token(token: Option<String>) -> Self {
        match token.as_deref() {
            Some(sh!("Info")) => Severity::Info,
            Some(sh!("Warning")) => Severity::Warning,
            Some(sh!("Violation")) | None => Severity::Violation,
            Some(other) => Severity::Other(strip_iri(other).unwrap_or(other).to_string()),
        }
    }

    pub fn iri(&self) -> String {
        match self {
            Severity::Info => format!("{SH}Info"),
            Severity::Warning => format!("{SH}Warning"),
            Severity::Violation => format!("{SH}Violation"),
            Severity::Other(iri) => iri.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationResult {
    pub focus_node: String,
    pub value_node: Option<String>,
    pub result_path: Option<String>,
    pub source_shape: String,
    pub source_constraint_component: String,
    pub severity: Severity,
    pub messages: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationReport {
    pub conforms: bool,
    pub results: Vec<ValidationResult>,
}

impl ValidationReport {
    pub fn to_json(&self) -> String {
        use serde_json::json;
        let results: Vec<_> = self
            .results
            .iter()
            .map(|r| {
                json!({
                    "focusNode": term_json_string(&r.focus_node),
                    "valueNode": r.value_node.as_deref().map(term_json_string),
                    "resultPath": r.result_path,
                    "sourceShape": term_json_string(&r.source_shape),
                    "sourceConstraintComponent": r.source_constraint_component,
                    "resultSeverity": r.severity.iri(),
                    "resultMessage": r.messages,
                })
            })
            .collect();
        serde_json::to_string_pretty(&json!({
            "conforms": self.conforms,
            "results": results,
        }))
        .unwrap_or_default()
    }

    pub fn to_turtle(&self) -> String {
        let mut out = String::new();
        out.push_str("@prefix sh: <http://www.w3.org/ns/shacl#> .\n\n");
        out.push_str("[] a <http://www.w3.org/ns/shacl#ValidationReport> ;\n");
        out.push_str(&format!(
            "   <http://www.w3.org/ns/shacl#conforms> {} ",
            self.conforms
        ));
        if self.results.is_empty() {
            out.push_str(".\n");
            return out;
        }
        out.push_str(";\n");
        for (i, r) in self.results.iter().enumerate() {
            out.push_str("   <http://www.w3.org/ns/shacl#result> [\n");
            out.push_str("      a <http://www.w3.org/ns/shacl#ValidationResult> ;\n");
            out.push_str(&format!(
                "      <http://www.w3.org/ns/shacl#focusNode> {} ;\n",
                r.focus_node
            ));
            if let Some(v) = &r.value_node {
                out.push_str(&format!("      <http://www.w3.org/ns/shacl#value> {v} ;\n"));
            }
            if let Some(path) = &r.result_path {
                out.push_str(&format!(
                    "      <http://www.w3.org/ns/shacl#resultPath> \"{}\" ;\n",
                    escape_string(path)
                ));
            }
            out.push_str(&format!(
                "      <http://www.w3.org/ns/shacl#sourceShape> {} ;\n",
                r.source_shape
            ));
            out.push_str(&format!(
                "      <http://www.w3.org/ns/shacl#sourceConstraintComponent> <{}> ;\n",
                r.source_constraint_component
            ));
            out.push_str(&format!(
                "      <http://www.w3.org/ns/shacl#resultSeverity> <{}>",
                r.severity.iri()
            ));
            for msg in &r.messages {
                out.push_str(&format!(
                    " ;\n      <http://www.w3.org/ns/shacl#resultMessage> \"{}\"",
                    escape_string(msg)
                ));
            }
            out.push_str("\n   ]");
            out.push_str(if i + 1 == self.results.len() {
                " .\n"
            } else {
                " ;\n"
            });
        }
        out
    }
}

#[derive(Debug, Clone)]
enum Path {
    Predicate(String),
    Inverse(Box<Path>),
    Sequence(Vec<Path>),
    Alternative(Vec<Path>),
    ZeroOrMore(Box<Path>),
    OneOrMore(Box<Path>),
    ZeroOrOne(Box<Path>),
}

impl Path {
    fn display(&self) -> String {
        match self {
            Path::Predicate(p) => p.clone(),
            Path::Inverse(p) => format!("^{}", p.display()),
            Path::Sequence(ps) => format!(
                "({})",
                ps.iter().map(Path::display).collect::<Vec<_>>().join(" ")
            ),
            Path::Alternative(ps) => format!(
                "({})",
                ps.iter().map(Path::display).collect::<Vec<_>>().join("|")
            ),
            Path::ZeroOrMore(p) => format!("{}*", p.display()),
            Path::OneOrMore(p) => format!("{}+", p.display()),
            Path::ZeroOrOne(p) => format!("{}?", p.display()),
        }
    }
}

#[derive(Debug)]
struct ShapeView<'a> {
    id: &'a str,
    path: Option<Path>,
    severity: Severity,
    messages: Vec<String>,
}

struct Validator<'a, G: GraphView> {
    data: &'a G,
    shapes: &'a ShaclShapes,
}

/// Validate `data` against `shapes`. `data` is any [`GraphView`] — an in-memory
/// [`DataGraph`] (eager) or a [`ReteGraph`] that routes lookups as range reads
/// (lazy / remote, fetching only the shapes' targets).
pub fn validate_shacl<G: GraphView>(data: &G, shapes: &ShaclShapes) -> ValidationReport {
    let validator = Validator { data, shapes };
    let mut results = Vec::new();
    for shape in shapes.target_shapes() {
        let targets = validator.targets(&shape);
        for focus in targets {
            results.extend(validator.validate_shape(&shape, &focus, &mut Vec::new()));
        }
    }
    results.sort_by(|a, b| {
        (
            &a.focus_node,
            &a.result_path,
            &a.source_constraint_component,
            &a.value_node,
        )
            .cmp(&(
                &b.focus_node,
                &b.result_path,
                &b.source_constraint_component,
                &b.value_node,
            ))
    });
    results.dedup();
    ValidationReport {
        conforms: results.is_empty(),
        results,
    }
}

impl<'a, G: GraphView> Validator<'a, G> {
    fn view(&self, shape: &'a str) -> ShapeView<'a> {
        let path = self
            .shapes
            .objects(shape, sh!("path"))
            .first()
            .and_then(|p| self.parse_path(p).ok());
        let severity =
            Severity::from_token(self.shapes.objects(shape, sh!("severity")).first().cloned());
        let messages = self
            .shapes
            .objects(shape, sh!("message"))
            .into_iter()
            .filter_map(|m| literal_lexical(&m).map(|l| l.value))
            .collect();
        ShapeView {
            id: shape,
            path,
            severity,
            messages,
        }
    }

    fn targets(&self, shape: &str) -> Vec<String> {
        let mut out = Vec::new();
        out.extend(self.shapes.objects(shape, sh!("targetNode")));
        for class in self.shapes.objects(shape, sh!("targetClass")) {
            out.extend(self.data.instances_of(&class));
        }
        for pred in self.shapes.objects(shape, sh!("targetSubjectsOf")) {
            out.extend(self.data.subjects_of(&pred));
        }
        for pred in self.shapes.objects(shape, sh!("targetObjectsOf")) {
            out.extend(self.data.objects_of(&pred));
        }
        if self.shapes.has(shape, RDF_TYPE, RDFS_CLASS)
            || self.shapes.has(shape, RDF_TYPE, OWL_CLASS)
        {
            out.extend(self.data.instances_of(shape));
        }
        unique(out)
    }

    fn validate_shape(
        &self,
        shape: &str,
        focus: &str,
        stack: &mut Vec<(String, String)>,
    ) -> Vec<ValidationResult> {
        if stack.iter().any(|(s, f)| s == shape && f == focus) {
            return vec![self.result(
                &self.view(shape),
                focus,
                None,
                component("RecursiveConstraintComponent"),
                None,
            )];
        }
        stack.push((shape.to_string(), focus.to_string()));
        let view = self.view(shape);
        if bool_param(self.shapes.objects(shape, sh!("deactivated")).first()) {
            stack.pop();
            return Vec::new();
        }
        let (values, result_path) = match &view.path {
            Some(path) => (self.eval_path(path, focus), Some(path.display())),
            None => (vec![focus.to_string()], None),
        };
        let mut out = Vec::new();

        self.check_cardinality(&view, focus, &values, result_path.as_deref(), &mut out);
        self.check_value_type(&view, focus, &values, result_path.as_deref(), &mut out);
        self.check_value_ranges(&view, focus, &values, result_path.as_deref(), &mut out);
        self.check_strings(&view, focus, &values, result_path.as_deref(), &mut out);
        self.check_property_pairs(&view, focus, &values, result_path.as_deref(), &mut out);
        self.check_has_value_and_in(&view, focus, &values, result_path.as_deref(), &mut out);
        self.check_nested_shapes(
            &view,
            focus,
            &values,
            result_path.as_deref(),
            stack,
            &mut out,
        );
        self.check_logical(&view, focus, stack, &mut out);
        self.check_closed(&view, focus, &mut out);
        self.check_qualified(
            &view,
            focus,
            &values,
            result_path.as_deref(),
            stack,
            &mut out,
        );

        stack.pop();
        out
    }

    fn conforms(&self, shape: &str, focus: &str, stack: &mut Vec<(String, String)>) -> bool {
        self.validate_shape(shape, focus, stack).is_empty()
    }

    fn check_cardinality(
        &self,
        view: &ShapeView<'_>,
        focus: &str,
        values: &[String],
        path: Option<&str>,
        out: &mut Vec<ValidationResult>,
    ) {
        for min in self.shapes.objects(view.id, sh!("minCount")) {
            if let Some(n) = int_literal(&min) {
                if values.len() < n as usize {
                    out.push(self.result(
                        view,
                        focus,
                        None,
                        component("MinCountConstraintComponent"),
                        path,
                    ));
                }
            }
        }
        for max in self.shapes.objects(view.id, sh!("maxCount")) {
            if let Some(n) = int_literal(&max) {
                if values.len() > n as usize {
                    out.push(self.result(
                        view,
                        focus,
                        None,
                        component("MaxCountConstraintComponent"),
                        path,
                    ));
                }
            }
        }
    }

    fn check_value_type(
        &self,
        view: &ShapeView<'_>,
        focus: &str,
        values: &[String],
        path: Option<&str>,
        out: &mut Vec<ValidationResult>,
    ) {
        for kind in self.shapes.objects(view.id, sh!("nodeKind")) {
            for v in values {
                if !node_kind(v, &kind) {
                    out.push(self.result(
                        view,
                        focus,
                        Some(v.clone()),
                        component("NodeKindConstraintComponent"),
                        path,
                    ));
                }
            }
        }
        for class in self.shapes.objects(view.id, sh!("class")) {
            for v in values {
                if !self.data.is_instance_of(v, &class) {
                    out.push(self.result(
                        view,
                        focus,
                        Some(v.clone()),
                        component("ClassConstraintComponent"),
                        path,
                    ));
                }
            }
        }
        for datatype in self.shapes.objects(view.id, sh!("datatype")) {
            for v in values {
                if !datatype_matches(v, &datatype) {
                    out.push(self.result(
                        view,
                        focus,
                        Some(v.clone()),
                        component("DatatypeConstraintComponent"),
                        path,
                    ));
                }
            }
        }
    }

    fn check_value_ranges(
        &self,
        view: &ShapeView<'_>,
        focus: &str,
        values: &[String],
        path: Option<&str>,
        out: &mut Vec<ValidationResult>,
    ) {
        let checks = [
            (sh!("minExclusive"), "MinExclusiveConstraintComponent", 0_u8),
            (sh!("minInclusive"), "MinInclusiveConstraintComponent", 1),
            (sh!("maxExclusive"), "MaxExclusiveConstraintComponent", 2),
            (sh!("maxInclusive"), "MaxInclusiveConstraintComponent", 3),
        ];
        for (pred, comp, mode) in checks {
            for bound in self.shapes.objects(view.id, pred) {
                for v in values {
                    let ok = compare_terms(v, &bound).is_some_and(|ord| match mode {
                        0 => ord.is_gt(),
                        1 => !ord.is_lt(),
                        2 => ord.is_lt(),
                        _ => !ord.is_gt(),
                    });
                    if !ok {
                        out.push(self.result(view, focus, Some(v.clone()), component(comp), path));
                    }
                }
            }
        }
    }

    fn check_strings(
        &self,
        view: &ShapeView<'_>,
        focus: &str,
        values: &[String],
        path: Option<&str>,
        out: &mut Vec<ValidationResult>,
    ) {
        for min in self.shapes.objects(view.id, sh!("minLength")) {
            if let Some(n) = int_literal(&min) {
                for v in values {
                    if string_value(v).chars().count() < n as usize {
                        out.push(self.result(
                            view,
                            focus,
                            Some(v.clone()),
                            component("MinLengthConstraintComponent"),
                            path,
                        ));
                    }
                }
            }
        }
        for max in self.shapes.objects(view.id, sh!("maxLength")) {
            if let Some(n) = int_literal(&max) {
                for v in values {
                    if string_value(v).chars().count() > n as usize {
                        out.push(self.result(
                            view,
                            focus,
                            Some(v.clone()),
                            component("MaxLengthConstraintComponent"),
                            path,
                        ));
                    }
                }
            }
        }
        for pattern in self.shapes.objects(view.id, sh!("pattern")) {
            let flags = self
                .shapes
                .objects(view.id, sh!("flags"))
                .first()
                .and_then(|f| literal_lexical(f).map(|l| l.value))
                .unwrap_or_default();
            let pat = literal_lexical(&pattern)
                .map(|l| l.value)
                .unwrap_or(pattern);
            let inline: String = ['i', 'm', 's', 'x']
                .iter()
                .filter(|c| flags.contains(**c))
                .collect();
            let full = if inline.is_empty() {
                pat
            } else {
                format!("(?{inline}){pat}")
            };
            let re = regex_lite::Regex::new(&full);
            for v in values {
                if re.as_ref().map_or(true, |r| !r.is_match(&string_value(v))) {
                    out.push(self.result(
                        view,
                        focus,
                        Some(v.clone()),
                        component("PatternConstraintComponent"),
                        path,
                    ));
                }
            }
        }
        for head in self.shapes.objects(view.id, sh!("languageIn")) {
            let allowed = self
                .shapes
                .list(&head)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|t| literal_lexical(&t).map(|l| l.value.to_ascii_lowercase()))
                .collect::<BTreeSet<_>>();
            for v in values {
                let lang = literal_lexical(v)
                    .and_then(|l| l.lang)
                    .map(|l| l.to_ascii_lowercase());
                if lang.is_none_or(|l| !allowed.contains("*") && !allowed.contains(&l)) {
                    out.push(self.result(
                        view,
                        focus,
                        Some(v.clone()),
                        component("LanguageInConstraintComponent"),
                        path,
                    ));
                }
            }
        }
        if bool_param(self.shapes.objects(view.id, sh!("uniqueLang")).first()) {
            let mut seen = BTreeSet::new();
            let mut duplicate = false;
            for v in values {
                if let Some(lang) = literal_lexical(v).and_then(|l| l.lang) {
                    if !seen.insert(lang.to_ascii_lowercase()) {
                        duplicate = true;
                    }
                }
            }
            if duplicate {
                out.push(self.result(
                    view,
                    focus,
                    None,
                    component("UniqueLangConstraintComponent"),
                    path,
                ));
            }
        }
    }

    fn check_property_pairs(
        &self,
        view: &ShapeView<'_>,
        focus: &str,
        values: &[String],
        path: Option<&str>,
        out: &mut Vec<ValidationResult>,
    ) {
        for other in self.shapes.objects(view.id, sh!("equals")) {
            let other_values = self.eval_path(&Path::Predicate(other), focus);
            if set(values) != set(&other_values) {
                out.push(self.result(
                    view,
                    focus,
                    None,
                    component("EqualsConstraintComponent"),
                    path,
                ));
            }
        }
        for other in self.shapes.objects(view.id, sh!("disjoint")) {
            let other_values = self.eval_path(&Path::Predicate(other), focus);
            if values.iter().any(|v| other_values.contains(v)) {
                out.push(self.result(
                    view,
                    focus,
                    None,
                    component("DisjointConstraintComponent"),
                    path,
                ));
            }
        }
        for other in self.shapes.objects(view.id, sh!("lessThan")) {
            let other_values = self.eval_path(&Path::Predicate(other), focus);
            for v in values {
                if other_values
                    .iter()
                    .any(|o| compare_terms(v, o).is_none_or(|ord| !ord.is_lt()))
                {
                    out.push(self.result(
                        view,
                        focus,
                        Some(v.clone()),
                        component("LessThanConstraintComponent"),
                        path,
                    ));
                }
            }
        }
        for other in self.shapes.objects(view.id, sh!("lessThanOrEquals")) {
            let other_values = self.eval_path(&Path::Predicate(other), focus);
            for v in values {
                if other_values
                    .iter()
                    .any(|o| compare_terms(v, o).is_none_or(|ord| ord.is_gt()))
                {
                    out.push(self.result(
                        view,
                        focus,
                        Some(v.clone()),
                        component("LessThanOrEqualsConstraintComponent"),
                        path,
                    ));
                }
            }
        }
    }

    fn check_has_value_and_in(
        &self,
        view: &ShapeView<'_>,
        focus: &str,
        values: &[String],
        path: Option<&str>,
        out: &mut Vec<ValidationResult>,
    ) {
        for required in self.shapes.objects(view.id, sh!("hasValue")) {
            if !values.contains(&required) {
                out.push(self.result(
                    view,
                    focus,
                    Some(required),
                    component("HasValueConstraintComponent"),
                    path,
                ));
            }
        }
        for head in self.shapes.objects(view.id, sh!("in")) {
            let allowed = self.shapes.list(&head).unwrap_or_default();
            for v in values {
                if !allowed.contains(v) {
                    out.push(self.result(
                        view,
                        focus,
                        Some(v.clone()),
                        component("InConstraintComponent"),
                        path,
                    ));
                }
            }
        }
    }

    fn check_nested_shapes(
        &self,
        view: &ShapeView<'_>,
        focus: &str,
        values: &[String],
        path: Option<&str>,
        stack: &mut Vec<(String, String)>,
        out: &mut Vec<ValidationResult>,
    ) {
        for node_shape in self.shapes.objects(view.id, sh!("node")) {
            for v in values {
                if !self.conforms(&node_shape, v, stack) {
                    out.push(self.result(
                        view,
                        focus,
                        Some(v.clone()),
                        component("NodeConstraintComponent"),
                        path,
                    ));
                }
            }
        }
        for property_shape in self.shapes.objects(view.id, sh!("property")) {
            out.extend(self.validate_shape(&property_shape, focus, stack));
        }
    }

    fn check_logical(
        &self,
        view: &ShapeView<'_>,
        focus: &str,
        stack: &mut Vec<(String, String)>,
        out: &mut Vec<ValidationResult>,
    ) {
        for s in self.shapes.objects(view.id, sh!("not")) {
            if self.conforms(&s, focus, stack) {
                out.push(self.result(
                    view,
                    focus,
                    Some(focus.to_string()),
                    component("NotConstraintComponent"),
                    None,
                ));
            }
        }
        for head in self.shapes.objects(view.id, sh!("and")) {
            let shapes = self.shapes.list(&head).unwrap_or_default();
            if shapes.iter().any(|s| !self.conforms(s, focus, stack)) {
                out.push(self.result(
                    view,
                    focus,
                    Some(focus.to_string()),
                    component("AndConstraintComponent"),
                    None,
                ));
            }
        }
        for head in self.shapes.objects(view.id, sh!("or")) {
            let shapes = self.shapes.list(&head).unwrap_or_default();
            if !shapes.iter().any(|s| self.conforms(s, focus, stack)) {
                out.push(self.result(
                    view,
                    focus,
                    Some(focus.to_string()),
                    component("OrConstraintComponent"),
                    None,
                ));
            }
        }
        for head in self.shapes.objects(view.id, sh!("xone")) {
            let shapes = self.shapes.list(&head).unwrap_or_default();
            let n = shapes
                .iter()
                .filter(|s| self.conforms(s, focus, stack))
                .count();
            if n != 1 {
                out.push(self.result(
                    view,
                    focus,
                    Some(focus.to_string()),
                    component("XoneConstraintComponent"),
                    None,
                ));
            }
        }
    }

    fn check_closed(&self, view: &ShapeView<'_>, focus: &str, out: &mut Vec<ValidationResult>) {
        if !bool_param(self.shapes.objects(view.id, sh!("closed")).first()) {
            return;
        }
        let mut allowed = BTreeSet::new();
        for prop_shape in self.shapes.objects(view.id, sh!("property")) {
            if let Some(path_node) = self.shapes.objects(&prop_shape, sh!("path")).first() {
                if is_iri(path_node) {
                    allowed.insert(path_node.clone());
                }
            }
        }
        for head in self.shapes.objects(view.id, sh!("ignoredProperties")) {
            for pred in self.shapes.list(&head).unwrap_or_default() {
                allowed.insert(pred);
            }
        }
        for pred in self.data.predicates_for_subject(focus) {
            if !allowed.contains(&pred) {
                out.push(self.result(
                    view,
                    focus,
                    Some(pred),
                    component("ClosedConstraintComponent"),
                    None,
                ));
            }
        }
    }

    fn check_qualified(
        &self,
        view: &ShapeView<'_>,
        focus: &str,
        values: &[String],
        path: Option<&str>,
        stack: &mut Vec<(String, String)>,
        out: &mut Vec<ValidationResult>,
    ) {
        let Some(qshape) = self
            .shapes
            .objects(view.id, sh!("qualifiedValueShape"))
            .first()
            .cloned()
        else {
            return;
        };
        let sibling_shapes = self.qualified_sibling_shapes(view.id, &qshape);
        let mut count = 0;
        for value in values {
            if !self.conforms(&qshape, value, stack) {
                continue;
            }
            if sibling_shapes
                .iter()
                .any(|sibling| self.conforms(sibling, value, stack))
            {
                continue;
            }
            count += 1;
        }
        for min in self.shapes.objects(view.id, sh!("qualifiedMinCount")) {
            if let Some(n) = int_literal(&min) {
                if count < n as usize {
                    out.push(self.result(
                        view,
                        focus,
                        None,
                        component("QualifiedMinCountConstraintComponent"),
                        path,
                    ));
                }
            }
        }
        for max in self.shapes.objects(view.id, sh!("qualifiedMaxCount")) {
            if let Some(n) = int_literal(&max) {
                if count > n as usize {
                    out.push(self.result(
                        view,
                        focus,
                        None,
                        component("QualifiedMaxCountConstraintComponent"),
                        path,
                    ));
                }
            }
        }
    }

    fn qualified_sibling_shapes(&self, property_shape: &str, qshape: &str) -> Vec<String> {
        if !bool_param(
            self.shapes
                .objects(property_shape, sh!("qualifiedValueShapesDisjoint"))
                .first(),
        ) {
            return Vec::new();
        }

        let mut siblings = Vec::new();
        for parent_shape in self.shapes.subjects(sh!("property"), property_shape) {
            for sibling_property_shape in self.shapes.objects(&parent_shape, sh!("property")) {
                siblings.extend(
                    self.shapes
                        .objects(&sibling_property_shape, sh!("qualifiedValueShape"))
                        .into_iter()
                        .filter(|sibling| sibling != qshape),
                );
            }
        }
        unique(siblings)
    }

    fn result(
        &self,
        view: &ShapeView<'_>,
        focus: &str,
        value: Option<String>,
        component: String,
        path: Option<&str>,
    ) -> ValidationResult {
        ValidationResult {
            focus_node: focus.to_string(),
            value_node: value,
            result_path: path.map(str::to_string),
            source_shape: view.id.to_string(),
            source_constraint_component: component,
            severity: view.severity.clone(),
            messages: view.messages.clone(),
        }
    }

    fn parse_path(&self, node: &str) -> Result<Path, ShaclError> {
        if is_iri(node) {
            return Ok(Path::Predicate(node.to_string()));
        }
        if let Some(p) = self.shapes.objects(node, sh!("inversePath")).first() {
            return Ok(Path::Inverse(Box::new(self.parse_path(p)?)));
        }
        if let Some(head) = self.shapes.objects(node, sh!("alternativePath")).first() {
            let paths = self
                .shapes
                .list(head)?
                .iter()
                .map(|n| self.parse_path(n))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Path::Alternative(paths));
        }
        if let Some(p) = self.shapes.objects(node, sh!("zeroOrMorePath")).first() {
            return Ok(Path::ZeroOrMore(Box::new(self.parse_path(p)?)));
        }
        if let Some(p) = self.shapes.objects(node, sh!("oneOrMorePath")).first() {
            return Ok(Path::OneOrMore(Box::new(self.parse_path(p)?)));
        }
        if let Some(p) = self.shapes.objects(node, sh!("zeroOrOnePath")).first() {
            return Ok(Path::ZeroOrOne(Box::new(self.parse_path(p)?)));
        }
        let paths = self
            .shapes
            .list(node)?
            .iter()
            .map(|n| self.parse_path(n))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Path::Sequence(paths))
    }

    fn eval_path(&self, path: &Path, start: &str) -> Vec<String> {
        match path {
            Path::Predicate(p) => unique(self.data.objects(start, p)),
            // The common inverse `^p` routes to `(?, p, start)` — one targeted
            // query — instead of scanning every node. A general inverse path
            // (`^(p1/p2)`) still enumerates all nodes (inherently non-targeted).
            Path::Inverse(inner) => match inner.as_ref() {
                Path::Predicate(p) => unique(self.data.subjects_with(p, start)),
                _ => unique(
                    self.data
                        .all_nodes()
                        .into_iter()
                        .filter(|n| self.eval_path(inner, n).contains(&start.to_string()))
                        .collect(),
                ),
            },
            Path::Sequence(paths) => {
                let mut frontier = vec![start.to_string()];
                for p in paths {
                    let mut next = Vec::new();
                    for n in &frontier {
                        next.extend(self.eval_path(p, n));
                    }
                    frontier = unique(next);
                }
                frontier
            }
            Path::Alternative(paths) => unique(
                paths
                    .iter()
                    .flat_map(|p| self.eval_path(p, start))
                    .collect::<Vec<_>>(),
            ),
            Path::ZeroOrOne(p) => {
                let mut out = vec![start.to_string()];
                out.extend(self.eval_path(p, start));
                unique(out)
            }
            Path::ZeroOrMore(p) => {
                let mut out = vec![start.to_string()];
                out.extend(self.transitive_path(p, start));
                unique(out)
            }
            Path::OneOrMore(p) => self.transitive_path(p, start),
        }
    }

    fn transitive_path(&self, path: &Path, start: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = self.eval_path(path, start);
        while let Some(n) = stack.pop() {
            if !seen.insert(n.clone()) {
                continue;
            }
            out.push(n.clone());
            stack.extend(self.eval_path(path, &n));
        }
        unique(out)
    }
}

fn component(local: &str) -> String {
    format!("{SH}{local}")
}

fn unique(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

fn set(values: &[String]) -> BTreeSet<String> {
    values.iter().cloned().collect()
}

use crate::terms::{iri_content as strip_iri, is_iri};

fn bool_param(v: Option<&String>) -> bool {
    v.is_some_and(|t| {
        literal_lexical(t)
            .map(|l| l.value == "true" || l.value == "1")
            .unwrap_or(false)
    })
}

fn int_literal(t: &str) -> Option<i64> {
    literal_lexical(t)?.value.parse().ok()
}

#[derive(Debug, Clone)]
struct Lit {
    value: String,
    datatype: Option<String>,
    lang: Option<String>,
}

fn literal_lexical(token: &str) -> Option<Lit> {
    if !token.starts_with('"') {
        return None;
    }
    let bytes = token.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => break,
            _ => i += 1,
        }
    }
    let value = unescape_nt(&token[1..i.min(token.len())]);
    let rest = token.get(i + 1..).unwrap_or("");
    let datatype = rest
        .strip_prefix("^^<")
        .and_then(|s| s.strip_suffix('>'))
        .map(|s| format!("<{s}>"));
    let lang = rest.strip_prefix('@').map(str::to_string);
    Some(Lit {
        value,
        datatype,
        lang,
    })
}

fn literal_datatype(token: &str) -> Option<String> {
    let lit = literal_lexical(token)?;
    if lit.lang.is_some() {
        Some(RDF_LANG_STRING.to_string())
    } else {
        Some(lit.datatype.unwrap_or_else(|| XSD_STRING.to_string()))
    }
}

fn datatype_matches(value: &str, datatype: &str) -> bool {
    literal_datatype(value).is_some_and(|dt| dt == datatype)
}

fn node_kind(value: &str, kind: &str) -> bool {
    match kind {
        sh!("IRI") => is_iri(value),
        sh!("BlankNode") => value.starts_with("_:"),
        sh!("Literal") => value.starts_with('"'),
        sh!("BlankNodeOrIRI") => value.starts_with("_:") || is_iri(value),
        sh!("BlankNodeOrLiteral") => value.starts_with("_:") || value.starts_with('"'),
        sh!("IRIOrLiteral") => is_iri(value) || value.starts_with('"'),
        _ => true,
    }
}

fn string_value(value: &str) -> String {
    if let Some(l) = literal_lexical(value) {
        l.value
    } else if let Some(iri) = strip_iri(value) {
        iri.to_string()
    } else {
        value.to_string()
    }
}

fn compare_terms(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let av = literal_lexical(a)
        .map(|l| l.value)
        .unwrap_or_else(|| string_value(a));
    let bv = literal_lexical(b)
        .map(|l| l.value)
        .unwrap_or_else(|| string_value(b));
    match (av.parse::<f64>(), bv.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y),
        _ => Some(av.cmp(&bv)),
    }
}

fn term_json_string(token: &str) -> String {
    strip_iri(token).unwrap_or(token).to_string()
}

fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn unescape_nt(s: &str) -> String {
    crate::terms::unescape_literal(s)
}
