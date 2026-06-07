//! A read-only **Cypher subset** that runs against `.rete` files by translating
//! Cypher to SPARQL and reusing [`rete_core::eval_query`].
//!
//! This is a *prototype*. It does not implement a second query engine: it parses
//! a small, documented subset of Cypher into an AST, emits an equivalent SPARQL
//! `SELECT` query string, and hands that to the existing SPARQL engine — so it
//! inherits BGP/join, property-path, and FILTER evaluation for free.
//!
//! # Supported subset (read-only)
//!
//! ```text
//! query      := MATCH patterns [WHERE conditions] RETURN items [LIMIT n]
//! patterns   := pattern ("," pattern)*
//! pattern    := node (rel node)*
//! node       := "(" [var] [":" Label] ")"
//! rel        := "-" "[" ":" REL ["*"] "]" "->"      (forward)
//!             | "<-" "[" ":" REL ["*"] "]" "-"      (reverse)
//! conditions := condition (("AND" | "OR") condition)*
//! condition  := var "." prop  OP  value             (property comparison)
//!             | var          "=" value              (identity)
//! OP         := "=" | "<>" | "!=" | "<" | "<=" | ">" | ">="
//! value      := number | "string" | <iri>
//! items      := item ("," item)*
//! item       := var | var "." prop
//! ```
//!
//! # Name → IRI convention
//!
//! A bare label/relationship/property name `X` maps to `<BASE + X>`, where `BASE`
//! defaults to `http://ex/` (overridable via `--base`). So with the default base:
//!
//! * `(a:Library)`        → `?a a <http://ex/Library>`
//! * `-[:dependsOn]->`    → predicate `<http://ex/dependsOn>`
//! * `a.name`             → `?a <http://ex/name> ?a_name`
//! * `(a)-[:dependsOn*]->(b)` → `?a <http://ex/dependsOn>+ ?b`
//!
//! # Out of scope (rejected with a clear error, never a panic)
//!
//! Writes (`CREATE`/`MERGE`/`SET`/`DELETE`), `OPTIONAL MATCH`, `WITH`,
//! aggregations, relationship variables/properties (`[r:REL {since: 2020}]`), and
//! multiple labels per node.

use std::fmt::Write as _;

use rete_core::{eval_query, QueryOutput, Rete};

/// The default base IRI for bare names (overridable with `--base`).
pub const DEFAULT_BASE: &str = "http://ex/";

/// A Cypher translation/parse error. Always carries what wasn't understood.
#[derive(Debug, thiserror::Error)]
#[error("Cypher error: {0}")]
pub struct CypherError(pub String);

impl CypherError {
    fn new(msg: impl Into<String>) -> Self {
        CypherError(msg.into())
    }
}

type Result<T> = std::result::Result<T, CypherError>;

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

/// A node pattern: an optional variable and an optional single label.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    var: Option<String>,
    label: Option<String>,
}

/// Relationship direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dir {
    /// `(a)-[:REL]->(b)` — edge points from the left node to the right node.
    Forward,
    /// `(a)<-[:REL]-(b)` — edge points from the right node to the left node.
    Reverse,
}

/// A relationship pattern between two adjacent nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Rel {
    rel_type: String,
    dir: Dir,
    /// Variable-length (`*`) → SPARQL `REL+` (one-or-more) for the prototype.
    var_length: bool,
}

/// One `MATCH` path: a node, then zero or more `(rel, node)` steps.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Path {
    first: Node,
    steps: Vec<(Rel, Node)>,
}

/// A comparison operator in `WHERE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    fn sparql(self) -> &'static str {
        match self {
            CmpOp::Eq => "=",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }
}

/// The left-hand side of a `WHERE` condition.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Lhs {
    /// `a.prop` — a node property.
    Prop(String, String),
    /// `a` — node identity.
    Ident(String),
}

/// A literal value on the right-hand side of a condition.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    Number(String),
    Str(String),
    /// An explicit `<iri>` literal.
    Iri(String),
}

/// A single `WHERE` condition.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Condition {
    lhs: Lhs,
    op: CmpOp,
    value: Value,
}

/// Conditions joined by a single boolean connective (kept flat for the subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoolOp {
    And,
    Or,
}

/// A `RETURN` item.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReturnItem {
    Var(String),
    Prop(String, String),
}

/// A fully parsed Cypher query.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CypherQuery {
    paths: Vec<Path>,
    /// `(conditions, connective)` — the connective joins all conditions; with one
    /// condition it is unused.
    where_: Vec<Condition>,
    where_op: BoolOp,
    returns: Vec<ReturnItem>,
    limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// An identifier or bare keyword (case-insensitive keywords handled later).
    Ident(String),
    /// A quoted string literal (contents, unescaped of surrounding quotes only).
    Str(String),
    /// A numeric literal (kept as text).
    Num(String),
    /// An `<iri>` literal including the angle brackets.
    Iri(String),
    /// Punctuation / multi-char operators.
    Punct(String),
}

/// Tokenize a Cypher query string. Recognizes identifiers, numbers, quoted
/// strings, `<iri>` literals, and the punctuation/operator set the grammar uses
/// (including multi-char `->`, `<-`, `<=`, `>=`, `<>`, `!=`).
fn tokenize(src: &str) -> Result<Vec<Tok>> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut toks = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // `<iri>` vs the `<-` / `<=` / `<>` operators: an `<` that begins an IRI
        // is followed by a non-operator char and contains a closing `>`.
        if c == '<' {
            // Try to read an IRI: `<...>` with no whitespace inside.
            if let Some(end) = find_iri_end(&chars, i) {
                let iri: String = chars[i..=end].iter().collect();
                toks.push(Tok::Iri(iri));
                i = end + 1;
                continue;
            }
            // Operators starting with `<`.
            if i + 1 < chars.len() && (chars[i + 1] == '-') {
                toks.push(Tok::Punct("<-".into()));
                i += 2;
                continue;
            }
            if i + 1 < chars.len() && (chars[i + 1] == '=') {
                toks.push(Tok::Punct("<=".into()));
                i += 2;
                continue;
            }
            if i + 1 < chars.len() && (chars[i + 1] == '>') {
                toks.push(Tok::Punct("<>".into()));
                i += 2;
                continue;
            }
            toks.push(Tok::Punct("<".into()));
            i += 1;
            continue;
        }
        if c == '-' {
            if i + 1 < chars.len() && chars[i + 1] == '>' {
                toks.push(Tok::Punct("->".into()));
                i += 2;
            } else {
                toks.push(Tok::Punct("-".into()));
                i += 1;
            }
            continue;
        }
        if c == '>' {
            if i + 1 < chars.len() && chars[i + 1] == '=' {
                toks.push(Tok::Punct(">=".into()));
                i += 2;
            } else {
                toks.push(Tok::Punct(">".into()));
                i += 1;
            }
            continue;
        }
        if c == '!' {
            if i + 1 < chars.len() && chars[i + 1] == '=' {
                toks.push(Tok::Punct("!=".into()));
                i += 2;
                continue;
            }
            return Err(CypherError::new("unexpected '!' (did you mean '!='?)"));
        }
        if c == '"' || c == '\'' {
            let quote = c;
            let mut s = String::new();
            i += 1;
            let mut closed = false;
            while i < chars.len() {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    // Pass through the escaped char (kept simple for the subset).
                    s.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                if chars[i] == quote {
                    closed = true;
                    i += 1;
                    break;
                }
                s.push(chars[i]);
                i += 1;
            }
            if !closed {
                return Err(CypherError::new("unterminated string literal"));
            }
            toks.push(Tok::Str(s));
            continue;
        }
        if c.is_ascii_digit() || (c == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
        {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            toks.push(Tok::Num(chars[start..i].iter().collect()));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            toks.push(Tok::Ident(chars[start..i].iter().collect()));
            continue;
        }
        if "()[]{}:,.*=".contains(c) {
            toks.push(Tok::Punct(c.to_string()));
            i += 1;
            continue;
        }
        return Err(CypherError::new(format!("unexpected character {c:?}")));
    }
    Ok(toks)
}

/// If a `<` at index `start` begins a well-formed `<iri>` (no whitespace, a
/// closing `>`), return the index of the `>`. Else `None` (it's an operator).
fn find_iri_end(chars: &[char], start: usize) -> Option<usize> {
    let mut j = start + 1;
    // An IRI body has no spaces and is not empty; `<-`/`<=`/`<>` are operators.
    if j >= chars.len() {
        return None;
    }
    if chars[j] == '-' || chars[j] == '=' || chars[j] == '>' {
        return None;
    }
    while j < chars.len() {
        match chars[j] {
            '>' => return Some(j),
            c if c.is_whitespace() => return None,
            _ => j += 1,
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn new(toks: Vec<Tok>) -> Self {
        Parser { toks, pos: 0 }
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    /// Consume an identifier matching `kw` case-insensitively.
    fn eat_keyword(&mut self, kw: &str) -> bool {
        if let Some(Tok::Ident(s)) = self.peek() {
            if s.eq_ignore_ascii_case(kw) {
                self.pos += 1;
                return true;
            }
        }
        false
    }

    /// True if the next token is the keyword `kw` (case-insensitive), no consume.
    fn is_keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(kw))
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Punct(s)) if s == p) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, p: &str) -> Result<()> {
        if self.eat_punct(p) {
            Ok(())
        } else {
            Err(CypherError::new(format!(
                "expected {p:?}, found {}",
                self.describe_next()
            )))
        }
    }

    fn describe_next(&self) -> String {
        match self.peek() {
            None => "end of input".to_string(),
            Some(Tok::Ident(s)) => format!("{s:?}"),
            Some(Tok::Str(s)) => format!("string {s:?}"),
            Some(Tok::Num(s)) => format!("number {s:?}"),
            Some(Tok::Iri(s)) => format!("IRI {s}"),
            Some(Tok::Punct(s)) => format!("{s:?}"),
        }
    }

    fn ident(&mut self) -> Result<String> {
        match self.next() {
            Some(Tok::Ident(s)) => Ok(s),
            other => Err(CypherError::new(format!(
                "expected a name, found {}",
                describe(other.as_ref())
            ))),
        }
    }

    fn parse_query(&mut self) -> Result<CypherQuery> {
        // Reject unsupported leading clauses with a clear message.
        for unsupported in ["CREATE", "MERGE", "SET", "DELETE", "WITH"] {
            if self.is_keyword(unsupported) {
                return Err(CypherError::new(format!(
                    "{unsupported} is not supported (this is a read-only Cypher subset)"
                )));
            }
        }
        if self.is_keyword("OPTIONAL") {
            return Err(CypherError::new("OPTIONAL MATCH is not supported"));
        }
        if !self.eat_keyword("MATCH") {
            return Err(CypherError::new(format!(
                "expected MATCH, found {}",
                self.describe_next()
            )));
        }
        let paths = self.parse_paths()?;

        let mut where_ = Vec::new();
        let mut where_op = BoolOp::And;
        if self.eat_keyword("WHERE") {
            let (conds, op) = self.parse_where()?;
            where_ = conds;
            where_op = op;
        }

        if !self.eat_keyword("RETURN") {
            return Err(CypherError::new(format!(
                "expected RETURN, found {}",
                self.describe_next()
            )));
        }
        let returns = self.parse_return()?;

        let mut limit = None;
        if self.eat_keyword("LIMIT") {
            match self.next() {
                Some(Tok::Num(n)) => {
                    let n: usize = n
                        .parse()
                        .map_err(|_| CypherError::new(format!("invalid LIMIT value {n:?}")))?;
                    limit = Some(n);
                }
                other => {
                    return Err(CypherError::new(format!(
                        "expected a number after LIMIT, found {}",
                        describe(other.as_ref())
                    )))
                }
            }
        }

        if let Some(t) = self.peek() {
            return Err(CypherError::new(format!(
                "unexpected trailing input: {}",
                describe(Some(t))
            )));
        }
        Ok(CypherQuery {
            paths,
            where_,
            where_op,
            returns,
            limit,
        })
    }

    fn parse_paths(&mut self) -> Result<Vec<Path>> {
        let mut paths = vec![self.parse_path()?];
        while self.eat_punct(",") {
            paths.push(self.parse_path()?);
        }
        Ok(paths)
    }

    fn parse_path(&mut self) -> Result<Path> {
        let first = self.parse_node()?;
        let mut steps = Vec::new();
        while self.at_rel_start() {
            let rel = self.parse_rel()?;
            let node = self.parse_node()?;
            steps.push((rel, node));
        }
        Ok(Path { first, steps })
    }

    /// True if the next tokens begin a relationship (`-[` or `<-[`).
    fn at_rel_start(&self) -> bool {
        matches!(self.peek(), Some(Tok::Punct(s)) if s == "-" || s == "<-")
    }

    fn parse_node(&mut self) -> Result<Node> {
        self.expect_punct("(")?;
        let mut var = None;
        let mut label = None;
        if let Some(Tok::Ident(_)) = self.peek() {
            var = Some(self.ident()?);
        }
        if self.eat_punct(":") {
            label = Some(self.ident()?);
            // A second label (`:A:B`) is out of scope.
            if self.eat_punct(":") {
                return Err(CypherError::new(
                    "multiple labels per node are not supported",
                ));
            }
        }
        // Inline node properties `{...}` are out of scope.
        if matches!(self.peek(), Some(Tok::Punct(s)) if s == "{") {
            return Err(CypherError::new(
                "inline node properties {{...}} are not supported",
            ));
        }
        self.expect_punct(")")?;
        Ok(Node { var, label })
    }

    fn parse_rel(&mut self) -> Result<Rel> {
        // Either `-[...]->` (forward) or `<-[...]-` (reverse).
        let dir = if self.eat_punct("<-") {
            Dir::Reverse
        } else if self.eat_punct("-") {
            Dir::Forward
        } else {
            return Err(CypherError::new(format!(
                "expected a relationship, found {}",
                self.describe_next()
            )));
        };
        self.expect_punct("[")?;
        // A relationship variable (`[r:REL]`) is out of scope — only `[:REL]`.
        if let Some(Tok::Ident(_)) = self.peek() {
            return Err(CypherError::new(
                "relationship variables ([r:REL]) are not supported; use [:REL]",
            ));
        }
        self.expect_punct(":")?;
        let rel_type = self.ident()?;
        let var_length = self.eat_punct("*");
        // `*N..M` bounds are out of scope; reject digits after `*` clearly.
        if var_length && matches!(self.peek(), Some(Tok::Num(_))) {
            return Err(CypherError::new(
                "bounded variable-length (*N..M) is not supported; use * (one-or-more)",
            ));
        }
        // Relationship properties `[:REL {since: 2020}]` are out of scope.
        if matches!(self.peek(), Some(Tok::Punct(s)) if s == "{") {
            return Err(CypherError::new(
                "relationship properties ([:REL {{...}}]) are not supported",
            ));
        }
        self.expect_punct("]")?;
        match dir {
            Dir::Forward => self.expect_punct("->")?,
            Dir::Reverse => self.expect_punct("-")?,
        }
        Ok(Rel {
            rel_type,
            dir,
            var_length,
        })
    }

    fn parse_where(&mut self) -> Result<(Vec<Condition>, BoolOp)> {
        let mut conds = vec![self.parse_condition()?];
        let mut op: Option<BoolOp> = None;
        loop {
            let next_op = if self.eat_keyword("AND") {
                BoolOp::And
            } else if self.eat_keyword("OR") {
                BoolOp::Or
            } else {
                break;
            };
            if let Some(prev) = op {
                if prev != next_op {
                    return Err(CypherError::new(
                        "mixing AND and OR is not supported; use one connective",
                    ));
                }
            }
            op = Some(next_op);
            conds.push(self.parse_condition()?);
        }
        Ok((conds, op.unwrap_or(BoolOp::And)))
    }

    fn parse_condition(&mut self) -> Result<Condition> {
        let var = self.ident()?;
        let lhs = if self.eat_punct(".") {
            let prop = self.ident()?;
            Lhs::Prop(var, prop)
        } else {
            Lhs::Ident(var)
        };
        let op = self.parse_cmp_op()?;
        let value = self.parse_value()?;
        // Identity conditions only make sense with = / != .
        if matches!(lhs, Lhs::Ident(_)) && !matches!(op, CmpOp::Eq | CmpOp::Ne) {
            return Err(CypherError::new(
                "node identity supports only = and <> comparisons",
            ));
        }
        Ok(Condition { lhs, op, value })
    }

    fn parse_cmp_op(&mut self) -> Result<CmpOp> {
        let op = match self.peek() {
            Some(Tok::Punct(s)) => match s.as_str() {
                "=" => CmpOp::Eq,
                "<>" | "!=" => CmpOp::Ne,
                "<" => CmpOp::Lt,
                "<=" => CmpOp::Le,
                ">" => CmpOp::Gt,
                ">=" => CmpOp::Ge,
                other => {
                    return Err(CypherError::new(format!(
                        "expected a comparison operator, found {other:?}"
                    )))
                }
            },
            other => {
                return Err(CypherError::new(format!(
                    "expected a comparison operator, found {}",
                    describe(other)
                )))
            }
        };
        self.pos += 1;
        Ok(op)
    }

    fn parse_value(&mut self) -> Result<Value> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(Value::Number(n)),
            Some(Tok::Str(s)) => Ok(Value::Str(s)),
            Some(Tok::Iri(iri)) => Ok(Value::Iri(iri)),
            other => Err(CypherError::new(format!(
                "expected a value (number, string, or <iri>), found {}",
                describe(other.as_ref())
            ))),
        }
    }

    fn parse_return(&mut self) -> Result<Vec<ReturnItem>> {
        if self.eat_punct("*") {
            return Err(CypherError::new(
                "RETURN * is not supported; list variables explicitly",
            ));
        }
        let mut items = vec![self.parse_return_item()?];
        while self.eat_punct(",") {
            items.push(self.parse_return_item()?);
        }
        Ok(items)
    }

    fn parse_return_item(&mut self) -> Result<ReturnItem> {
        let var = self.ident()?;
        if self.eat_punct(".") {
            let prop = self.ident()?;
            Ok(ReturnItem::Prop(var, prop))
        } else {
            Ok(ReturnItem::Var(var))
        }
    }
}

fn describe(t: Option<&Tok>) -> String {
    match t {
        None => "end of input".to_string(),
        Some(Tok::Ident(s)) => format!("{s:?}"),
        Some(Tok::Str(s)) => format!("string {s:?}"),
        Some(Tok::Num(s)) => format!("number {s:?}"),
        Some(Tok::Iri(s)) => format!("IRI {s}"),
        Some(Tok::Punct(s)) => format!("{s:?}"),
    }
}

// ---------------------------------------------------------------------------
// Translation: AST → SPARQL
// ---------------------------------------------------------------------------

/// Render `BASE + name` as an `<iri>` token (assumes `name` needs no escaping —
/// Cypher identifiers are alphanumeric/underscore so this is safe).
fn iri(base: &str, name: &str) -> String {
    format!("<{base}{name}>")
}

/// Generate a fresh, deterministic variable name for anonymous nodes/properties.
struct VarGen {
    counter: usize,
}

impl VarGen {
    fn new() -> Self {
        VarGen { counter: 0 }
    }
    fn fresh(&mut self, hint: &str) -> String {
        let v = format!("__{hint}{}", self.counter);
        self.counter += 1;
        v
    }
}

/// Translate a parsed Cypher query into an equivalent SPARQL `SELECT` string.
fn translate(q: &CypherQuery, base: &str) -> Result<String> {
    let mut gen = VarGen::new();
    let mut triples: Vec<String> = Vec::new();

    // Each node's SPARQL variable (named or freshly generated for anonymous).
    let node_var = |n: &Node, gen: &mut VarGen| -> String {
        match &n.var {
            Some(v) => format!("?{v}"),
            None => format!("?{}", gen.fresh("n")),
        }
    };

    for path in &q.paths {
        // Build per-step, materializing each node's variable once. A named var is
        // stable across the path; anonymous nodes get a fresh var each.
        let mut prev_var = node_var(&path.first, &mut gen);
        emit_node_label(&path.first, &prev_var, base, &mut triples);

        for (rel, node) in &path.steps {
            let cur_var = node_var(node, &mut gen);
            emit_node_label(node, &cur_var, base, &mut triples);

            let pred = iri(base, &rel.rel_type);
            let pred = if rel.var_length {
                format!("{pred}+")
            } else {
                pred
            };
            match rel.dir {
                Dir::Forward => triples.push(format!("{prev_var} {pred} {cur_var} .")),
                Dir::Reverse => triples.push(format!("{cur_var} {pred} {prev_var} .")),
            }
            prev_var = cur_var;
        }
    }

    // RETURN property items need a triple binding `?a <base+prop> ?a_prop`.
    let mut projection: Vec<String> = Vec::new();
    for item in &q.returns {
        match item {
            ReturnItem::Var(v) => projection.push(format!("?{v}")),
            ReturnItem::Prop(v, prop) => {
                let pvar = format!("?{v}_{prop}");
                triples.push(format!("?{v} {} {pvar} .", iri(base, prop)));
                projection.push(pvar);
            }
        }
    }

    // WHERE → FILTER. Property comparisons need the property bound as a variable.
    let mut filters: Vec<String> = Vec::new();
    for cond in &q.where_ {
        match &cond.lhs {
            Lhs::Prop(v, prop) => {
                let pvar = format!("?__w_{v}_{prop}");
                triples.push(format!("?{v} {} {pvar} .", iri(base, prop)));
                filters.push(format!(
                    "{pvar} {} {}",
                    cond.op.sparql(),
                    value_to_sparql(&cond.value)
                ));
            }
            Lhs::Ident(v) => {
                filters.push(format!(
                    "?{v} {} {}",
                    cond.op.sparql(),
                    value_to_sparql(&cond.value)
                ));
            }
        }
    }

    let mut sparql = String::new();
    write!(sparql, "SELECT {} WHERE {{", projection.join(" ")).unwrap();
    for t in &triples {
        write!(sparql, " {t}").unwrap();
    }
    if !filters.is_empty() {
        let joiner = match q.where_op {
            BoolOp::And => " && ",
            BoolOp::Or => " || ",
        };
        write!(sparql, " FILTER({})", filters.join(joiner)).unwrap();
    }
    sparql.push_str(" }");
    if let Some(n) = q.limit {
        write!(sparql, " LIMIT {n}").unwrap();
    }
    Ok(sparql)
}

/// Emit the `?n a <base+Label>` triple for a labeled node (no-op if unlabeled).
fn emit_node_label(node: &Node, var: &str, base: &str, triples: &mut Vec<String>) {
    if let Some(label) = &node.label {
        triples.push(format!("{var} a {} .", iri(base, label)));
    }
}

/// Render a value as a SPARQL term: numbers verbatim, strings quoted, IRIs as-is.
fn value_to_sparql(v: &Value) -> String {
    match v {
        Value::Number(n) => n.clone(),
        Value::Str(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        Value::Iri(iri) => iri.clone(),
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Translate a Cypher query string into an equivalent SPARQL `SELECT` string,
/// using `base` for the bare-name → IRI convention. Returns a clear error (never
/// a panic) if the query is outside the supported subset.
pub fn cypher_to_sparql(cypher: &str, base: &str) -> Result<String> {
    let toks = tokenize(cypher)?;
    if toks.is_empty() {
        return Err(CypherError::new("empty query"));
    }
    let mut parser = Parser::new(toks);
    let query = parser.parse_query()?;
    translate(&query, base)
}

/// Translate a Cypher query to SPARQL and evaluate it against an open `.rete`.
pub fn eval_cypher(rete: &Rete, cypher: &str, base: &str) -> Result<QueryOutput> {
    let sparql = cypher_to_sparql(cypher, base)?;
    eval_query(rete, &sparql)
        .map_err(|e| CypherError::new(format!("{e} (translated to: {sparql})")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(cypher: &str) -> String {
        cypher_to_sparql(cypher, DEFAULT_BASE).unwrap()
    }

    #[test]
    fn translate_labeled_node_match() {
        assert_eq!(
            tr("MATCH (a:Library) RETURN a"),
            "SELECT ?a WHERE { ?a a <http://ex/Library> . }"
        );
    }

    #[test]
    fn translate_relationship() {
        assert_eq!(
            tr("MATCH (a)-[:dependsOn]->(b) RETURN a, b"),
            "SELECT ?a ?b WHERE { ?a <http://ex/dependsOn> ?b . }"
        );
    }

    #[test]
    fn translate_reverse_relationship() {
        assert_eq!(
            tr("MATCH (a)<-[:dependsOn]-(b) RETURN a, b"),
            "SELECT ?a ?b WHERE { ?b <http://ex/dependsOn> ?a . }"
        );
    }

    #[test]
    fn translate_variable_length_to_plus_path() {
        assert_eq!(
            tr("MATCH (a)-[:dependsOn*]->(b) RETURN a, b"),
            "SELECT ?a ?b WHERE { ?a <http://ex/dependsOn>+ ?b . }"
        );
    }

    #[test]
    fn translate_where_filter() {
        assert_eq!(
            tr("MATCH (a:Person) WHERE a.age > 30 RETURN a"),
            "SELECT ?a WHERE { ?a a <http://ex/Person> . ?a <http://ex/age> ?__w_a_age . \
             FILTER(?__w_a_age > 30) }"
        );
    }

    #[test]
    fn translate_identity_filter() {
        assert_eq!(
            tr("MATCH (a)-[:knows]->(b) WHERE a = <http://ex/Alice> RETURN b"),
            "SELECT ?b WHERE { ?a <http://ex/knows> ?b . FILTER(?a = <http://ex/Alice>) }"
        );
    }

    #[test]
    fn translate_string_property_filter() {
        assert_eq!(
            tr(r#"MATCH (a) WHERE a.name = "Alice" RETURN a"#),
            "SELECT ?a WHERE { ?a <http://ex/name> ?__w_a_name . FILTER(?__w_a_name = \"Alice\") }"
        );
    }

    #[test]
    fn translate_return_property_and_limit() {
        assert_eq!(
            tr("MATCH (a:Person) RETURN a, a.name LIMIT 5"),
            "SELECT ?a ?a_name WHERE { ?a a <http://ex/Person> . ?a <http://ex/name> ?a_name . } \
             LIMIT 5"
        );
    }

    #[test]
    fn translate_and_conditions() {
        assert_eq!(
            tr("MATCH (a) WHERE a.age > 18 AND a.age < 65 RETURN a"),
            "SELECT ?a WHERE { ?a <http://ex/age> ?__w_a_age . ?a <http://ex/age> ?__w_a_age . \
             FILTER(?__w_a_age > 18 && ?__w_a_age < 65) }"
        );
    }

    #[test]
    fn translate_multiple_patterns() {
        assert_eq!(
            tr("MATCH (a)-[:knows]->(b), (b)-[:knows]->(c) RETURN a, c"),
            "SELECT ?a ?c WHERE { ?a <http://ex/knows> ?b . ?b <http://ex/knows> ?c . }"
        );
    }

    #[test]
    fn custom_base_iri() {
        assert_eq!(
            cypher_to_sparql("MATCH (a:Library) RETURN a", "http://my/").unwrap(),
            "SELECT ?a WHERE { ?a a <http://my/Library> . }"
        );
    }

    // --- error handling: clear messages, never panics ---------------------

    #[test]
    fn rejects_writes() {
        let err = cypher_to_sparql("CREATE (a:Person) RETURN a", DEFAULT_BASE).unwrap_err();
        assert!(err.to_string().contains("CREATE"), "{err}");
    }

    #[test]
    fn rejects_relationship_variable() {
        let err = cypher_to_sparql("MATCH (a)-[r:knows]->(b) RETURN a", DEFAULT_BASE).unwrap_err();
        assert!(err.to_string().contains("relationship variable"), "{err}");
    }

    #[test]
    fn rejects_relationship_property() {
        let err = cypher_to_sparql(
            "MATCH (a)-[:knows {since: 2020}]->(b) RETURN a",
            DEFAULT_BASE,
        )
        .unwrap_err();
        assert!(err.to_string().contains("relationship properties"), "{err}");
    }

    #[test]
    fn rejects_multiple_labels() {
        let err = cypher_to_sparql("MATCH (a:Person:Admin) RETURN a", DEFAULT_BASE).unwrap_err();
        assert!(err.to_string().contains("multiple labels"), "{err}");
    }

    #[test]
    fn rejects_missing_return() {
        let err = cypher_to_sparql("MATCH (a:Person)", DEFAULT_BASE).unwrap_err();
        assert!(err.to_string().contains("RETURN"), "{err}");
    }

    #[test]
    fn rejects_garbage() {
        let err = cypher_to_sparql("not a query at all", DEFAULT_BASE).unwrap_err();
        assert!(err.to_string().contains("MATCH"), "{err}");
    }

    #[test]
    fn rejects_optional_match() {
        let err = cypher_to_sparql("OPTIONAL MATCH (a) RETURN a", DEFAULT_BASE).unwrap_err();
        assert!(err.to_string().contains("OPTIONAL"), "{err}");
    }

    // --- end-to-end: translate + eval against a built graph ----------------

    fn build_deps() -> Vec<u8> {
        use rete_core::{
            build_pyramid_meta, write_dataset, DictionaryBuilder, GraphIndexBuilder,
            DEFAULT_TILE_BUDGET,
        };
        // The dependency graph from examples/deps.nt (subset sufficient for tests).
        let triples_nt: &[(&str, &str, &str)] = &[
            (
                "<http://ex/app>",
                "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>",
                "<http://ex/Application>",
            ),
            (
                "<http://ex/web>",
                "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>",
                "<http://ex/Library>",
            ),
            (
                "<http://ex/auth>",
                "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>",
                "<http://ex/Library>",
            ),
            (
                "<http://ex/logging>",
                "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>",
                "<http://ex/Library>",
            ),
            (
                "<http://ex/log4x>",
                "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>",
                "<http://ex/Library>",
            ),
            (
                "<http://ex/log4x>",
                "<http://ex/hasVulnerability>",
                "<http://ex/CVE-2099-0001>",
            ),
            (
                "<http://ex/app>",
                "<http://ex/dependsOn>",
                "<http://ex/web>",
            ),
            (
                "<http://ex/app>",
                "<http://ex/dependsOn>",
                "<http://ex/auth>",
            ),
            (
                "<http://ex/web>",
                "<http://ex/dependsOn>",
                "<http://ex/logging>",
            ),
            (
                "<http://ex/auth>",
                "<http://ex/dependsOn>",
                "<http://ex/logging>",
            ),
            (
                "<http://ex/logging>",
                "<http://ex/dependsOn>",
                "<http://ex/log4x>",
            ),
        ];
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples_nt {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut def = GraphIndexBuilder::new();
        let mut encoded = Vec::new();
        for (s, p, o) in triples_nt {
            let t = dict.encode(s, p, o).unwrap();
            def.push(t);
            encoded.push(t);
        }
        let (meta, levels) = build_pyramid_meta(&dict, &encoded, DEFAULT_TILE_BUDGET);
        write_dataset(&dict, &def.build(), &[], false, &meta, levels)
    }

    fn rows(out: &QueryOutput) -> Vec<std::collections::BTreeMap<String, String>> {
        match out {
            QueryOutput::Select(_, sols) => sols
                .iter()
                .map(|b| b.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .collect(),
            _ => panic!("expected SELECT output"),
        }
    }

    #[test]
    fn end_to_end_labeled_match() {
        let bytes = build_deps();
        let rete = Rete::open(&bytes).unwrap();
        let out = eval_cypher(&rete, "MATCH (a:Application) RETURN a", DEFAULT_BASE).unwrap();
        let r = rows(&out);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["a"], "<http://ex/app>");
    }

    #[test]
    fn end_to_end_variable_length_path() {
        let bytes = build_deps();
        let rete = Rete::open(&bytes).unwrap();
        // What transitively depends on log4x? (reverse-style: a -[:dependsOn*]-> log4x)
        let out = eval_cypher(
            &rete,
            "MATCH (a)-[:dependsOn*]->(b) WHERE b = <http://ex/log4x> RETURN a",
            DEFAULT_BASE,
        )
        .unwrap();
        let mut got: Vec<String> = rows(&out).into_iter().map(|m| m["a"].clone()).collect();
        got.sort();
        // app → web/auth → logging → log4x : everything but log4x itself reaches it.
        assert_eq!(
            got,
            vec![
                "<http://ex/app>".to_string(),
                "<http://ex/auth>".to_string(),
                "<http://ex/logging>".to_string(),
                "<http://ex/web>".to_string(),
            ]
        );
    }

    #[test]
    fn end_to_end_direct_relationship() {
        let bytes = build_deps();
        let rete = Rete::open(&bytes).unwrap();
        let out = eval_cypher(
            &rete,
            "MATCH (a)-[:dependsOn]->(b) WHERE a = <http://ex/app> RETURN b",
            DEFAULT_BASE,
        )
        .unwrap();
        let mut got: Vec<String> = rows(&out).into_iter().map(|m| m["b"].clone()).collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                "<http://ex/auth>".to_string(),
                "<http://ex/web>".to_string()
            ]
        );
    }
}
