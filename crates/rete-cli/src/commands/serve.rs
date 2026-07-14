//! `rete serve` — a minimal **SPARQL 1.1 Protocol endpoint over one `.rete`**,
//! including SPARQL **Update**.
//!
//! The base file is **never mutated** (it stays a content-hashed, CDN-servable
//! artifact). Updates append to a plain-text **journal** next to it
//! (`<file>.changes`: one `+`/`-`-prefixed N-Quads line per change) and mutate
//! an in-memory quad set; the queryable image is rebuilt from that set lazily
//! (dirty flag), so update bursts coalesce into one rebuild. Restart = base +
//! journal replay. `GET /snapshot.rete` serializes the *current* state as a
//! fresh `.rete` — the downloadable companion; publishing an update cycle is
//! "upload the snapshot, delete the journal".
//!
//! This makes the write path a small, single-writer authority while every
//! *read* path stays serverless — and closes the federation loop: any rete
//! client (CLI, browser) can `SERVICE <http://host:port/sparql>` against it.
//!
//! Scale note: the whole graph lives in memory and rebuilds on write — right
//! for the living-dataset use case (annotation, curation, small/medium KGs, up
//! to a few million triples), not for the multi-GB catalog files (their story
//! remains offline rebuild/compaction; an engine-level overlay is future work).

use std::collections::BTreeSet;
use std::io::Write as _;

use rete_core::ingest::{assemble_dataset_with_opts, parse_quads, RawQuad};
use rete_core::{eval_query, sparql_json_ask, sparql_json_results, QueryOutput, Rete};
use spargebra::term::{
    GraphName, GraphNamePattern, GroundQuadPattern, GroundTermPattern, NamedNodePattern,
    QuadPattern, TermPattern,
};
use spargebra::{GraphUpdateOperation, Query, Update};

use super::service_http::HttpServiceClient;

/// One quad as term tokens; `None` graph = the default graph. `BTreeSet` gives
/// set semantics (RDF graphs are sets) and a deterministic rebuild order.
type QuadSet = BTreeSet<RawQuad>;

/// The server's whole state: the quad set (the truth), the current queryable
/// image built from it, and the journal that makes the set durable.
struct Store {
    quads: QuadSet,
    /// The serialized `.rete` of the current state (served by `/snapshot.rete`).
    image: Vec<u8>,
    rete: Rete,
    /// Set by updates; the next read rebuilds `image`/`rete` from `quads`.
    dirty: bool,
    journal: std::path::PathBuf,
    /// Counter for minting fresh blank nodes in INSERT templates.
    bnode_seq: u64,
}

/// Render a quad back to one N-Quads line (terms are already canonical tokens).
fn nq_line(q: &RawQuad) -> String {
    match &q.3 {
        Some(g) => format!("{} {} {} {} .", q.0, q.1, q.2, g),
        None => format!("{} {} {} .", q.0, q.1, q.2),
    }
}

/// Parse one journal payload (an N-Quads line) back to a quad.
fn parse_nq_line(line: &str) -> anyhow::Result<RawQuad> {
    let mut quads = parse_quads(line)?;
    match quads.len() {
        1 => Ok(quads.pop().expect("len checked")),
        n => anyhow::bail!("journal line held {n} statements: {line}"),
    }
}

impl Store {
    /// Open the base file, extract every quad, replay the journal, build the
    /// first image.
    fn open(base: &str, journal: std::path::PathBuf) -> anyhow::Result<Store> {
        let bytes = std::fs::read(base)?;
        let rete = Rete::open(&bytes)?;
        let mut quads: QuadSet = rete
            .dump(None)
            .into_iter()
            .map(|(s, p, o)| (s, p, o, None))
            .collect();
        let graphs: Vec<String> = rete.graph_names().iter().map(|g| g.to_string()).collect();
        for g in graphs {
            for (s, p, o) in rete.dump(Some(&g)) {
                quads.insert((s, p, o, Some(g.clone())));
            }
        }
        drop(rete);

        // Replay the journal (if any): each line is `+ <nq>` or `- <nq>`.
        let mut replayed = 0usize;
        if journal.exists() {
            for (i, line) in std::fs::read_to_string(&journal)?.lines().enumerate() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let (op, payload) = line.split_at(1);
                let quad = parse_nq_line(payload.trim_start())
                    .map_err(|e| anyhow::anyhow!("{}:{}: {e}", journal.display(), i + 1))?;
                match op {
                    "+" => {
                        quads.insert(quad);
                    }
                    "-" => {
                        quads.remove(&quad);
                    }
                    other => anyhow::bail!(
                        "{}:{}: unknown journal op {other:?} (expected + or -)",
                        journal.display(),
                        i + 1
                    ),
                }
                replayed += 1;
            }
        }

        let mut store = Store {
            quads,
            image: Vec::new(),
            rete: Rete::open(&bytes)?, // placeholder; rebuilt below
            dirty: true,
            journal,
            bnode_seq: 0,
        };
        store.ensure_current()?;
        if replayed > 0 {
            eprintln!("  replayed {replayed} journal change(s)");
        }
        Ok(store)
    }

    /// Rebuild the queryable image from the quad set if updates are pending.
    /// Pyramid off: the endpoint serves SPARQL, and skipping it makes the
    /// after-write rebuild fast.
    fn ensure_current(&mut self) -> anyhow::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let quads: Vec<RawQuad> = self.quads.iter().cloned().collect();
        let (image, _stats) =
            assemble_dataset_with_opts(quads, false, false, None, |_, _| Vec::new());
        let mut rete = Rete::open(&image)?;
        // The endpoint itself supports SERVICE blocks in incoming queries.
        rete.set_service_client(Box::new(HttpServiceClient));
        self.image = image;
        self.rete = rete;
        self.dirty = false;
        Ok(())
    }

    /// Apply one change to the set; journal it only if it actually changed
    /// something (set semantics: double-inserts and misses are no-ops).
    fn apply(&mut self, add: bool, quad: RawQuad, journal: &mut Vec<String>) {
        let changed = if add {
            self.quads.insert(quad.clone())
        } else {
            self.quads.remove(&quad)
        };
        if changed {
            journal.push(format!(
                "{} {}",
                if add { "+" } else { "-" },
                nq_line(&quad)
            ));
            self.dirty = true;
        }
    }

    /// Execute a SPARQL UPDATE request: operations run in order, each seeing
    /// the previous one's effects; every net change is journaled. Returns the
    /// number of changes applied.
    fn update(&mut self, text: &str) -> anyhow::Result<usize> {
        let parsed = Update::parse(text, None).map_err(|e| anyhow::anyhow!("parse: {e}"))?;
        let mut journal: Vec<String> = Vec::new();
        for op in &parsed.operations {
            self.run_op(op, &mut journal)?;
        }
        if !journal.is_empty() {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.journal)?;
            for line in &journal {
                writeln!(f, "{line}")?;
            }
            f.sync_all()?;
        }
        Ok(journal.len())
    }

    fn run_op(
        &mut self,
        op: &GraphUpdateOperation,
        journal: &mut Vec<String>,
    ) -> anyhow::Result<()> {
        match op {
            GraphUpdateOperation::InsertData { data } => {
                for q in data {
                    let quad = (
                        q.subject.to_string(),
                        q.predicate.to_string(),
                        q.object.to_string(),
                        graph_token(&q.graph_name),
                    );
                    self.apply(true, quad, journal);
                }
                Ok(())
            }
            GraphUpdateOperation::DeleteData { data } => {
                for q in data {
                    let quad = (
                        q.subject.to_string(),
                        q.predicate.to_string(),
                        q.object.to_string(),
                        graph_token(&q.graph_name),
                    );
                    self.apply(false, quad, journal);
                }
                Ok(())
            }
            GraphUpdateOperation::DeleteInsert {
                delete,
                insert,
                using,
                pattern,
            } => {
                if using.is_some() {
                    anyhow::bail!("USING is not supported");
                }
                // The WHERE pattern evaluates against the current state
                // (including earlier operations of this request).
                self.ensure_current()?;
                let query = Query::Select {
                    dataset: None,
                    pattern: (**pattern).clone(),
                    base_iri: None,
                }
                .to_string();
                let (_vars, solutions) = rete_core::eval_sparql(&self.rete, &query)
                    .map_err(|e| anyhow::anyhow!("WHERE evaluation: {e}"))?;
                // Deletes for every solution first, then inserts (per the spec).
                for sol in &solutions {
                    for tpl in delete {
                        if let Some(quad) = instantiate_ground(tpl, sol) {
                            self.apply(false, quad, journal);
                        }
                    }
                }
                for sol in &solutions {
                    // Blank nodes in INSERT templates mint fresh nodes per
                    // solution (never reuse a label from the data).
                    let mut minted: std::collections::HashMap<String, String> =
                        std::collections::HashMap::new();
                    for tpl in insert {
                        if let Some(quad) = self.instantiate(tpl, sol, &mut minted) {
                            self.apply(true, quad, journal);
                        }
                    }
                }
                Ok(())
            }
            GraphUpdateOperation::Clear { graph, .. }
            | GraphUpdateOperation::Drop { graph, .. } => {
                use spargebra::algebra::GraphTarget;
                let doomed: Vec<RawQuad> = self
                    .quads
                    .iter()
                    .filter(|q| match graph {
                        GraphTarget::DefaultGraph => q.3.is_none(),
                        GraphTarget::NamedNode(n) => q.3.as_deref() == Some(n.to_string().as_str()),
                        GraphTarget::NamedGraphs => q.3.is_some(),
                        GraphTarget::AllGraphs => true,
                    })
                    .cloned()
                    .collect();
                for quad in doomed {
                    self.apply(false, quad, journal);
                }
                Ok(())
            }
            // Graphs exist implicitly (a graph is its quads), so CREATE is a
            // no-op that succeeds.
            GraphUpdateOperation::Create { .. } => Ok(()),
            GraphUpdateOperation::Load { .. } => {
                anyhow::bail!("LOAD is not supported (ingest with `rete build`)")
            }
        }
    }

    /// Instantiate an INSERT template: like the ground case, plus blank nodes
    /// mint one fresh node per (solution, label).
    fn instantiate(
        &mut self,
        tpl: &QuadPattern,
        sol: &rete_core::Binding,
        minted: &mut std::collections::HashMap<String, String>,
    ) -> Option<RawQuad> {
        let mut term = |t: &TermPattern| -> Option<String> {
            match t {
                TermPattern::NamedNode(n) => Some(n.to_string()),
                TermPattern::Literal(l) => Some(l.to_string()),
                TermPattern::Variable(v) => sol.get(v.as_str()).cloned(),
                TermPattern::BlankNode(b) => Some(
                    minted
                        .entry(b.to_string())
                        .or_insert_with(|| {
                            self.bnode_seq += 1;
                            format!("_:m{}", self.bnode_seq)
                        })
                        .clone(),
                ),
                // RDF-star quoted triple in a template: resolve recursively to the
                // canonical `<< s p o >>` token (blank nodes inside a quoted triple
                // are not minted — skip if present).
                TermPattern::Triple(_) => quoted_term_token(t, sol),
            }
        };
        Some((
            term(&tpl.subject)?,
            named_term(&tpl.predicate, sol)?,
            term(&tpl.object)?,
            graph_pattern_token(&tpl.graph_name, sol)?,
        ))
    }
}

fn graph_token(g: &GraphName) -> Option<String> {
    match g {
        GraphName::DefaultGraph => None,
        GraphName::NamedNode(n) => Some(n.to_string()),
    }
}

/// Instantiate a ground (DELETE) template against one solution; `None` (skip
/// the quad) when a template variable is unbound in this solution.
fn instantiate_ground(tpl: &GroundQuadPattern, sol: &rete_core::Binding) -> Option<RawQuad> {
    Some((
        ground_term(&tpl.subject, sol)?,
        named_term(&tpl.predicate, sol)?,
        ground_term(&tpl.object, sol)?,
        graph_pattern_token(&tpl.graph_name, sol)?,
    ))
}

/// Resolve a template graph name; `Some(None)` = the default graph, outer
/// `None` = an unbound variable (skip the quad).
fn graph_pattern_token(g: &GraphNamePattern, sol: &rete_core::Binding) -> Option<Option<String>> {
    match g {
        GraphNamePattern::DefaultGraph => Some(None),
        GraphNamePattern::NamedNode(n) => Some(Some(n.to_string())),
        GraphNamePattern::Variable(v) => sol.get(v.as_str()).cloned().map(Some),
    }
}

fn ground_term(t: &GroundTermPattern, sol: &rete_core::Binding) -> Option<String> {
    match t {
        GroundTermPattern::NamedNode(n) => Some(n.to_string()),
        GroundTermPattern::Literal(l) => Some(l.to_string()),
        GroundTermPattern::Variable(v) => sol.get(v.as_str()).cloned(),
        // RDF-star quoted triple in a ground (DELETE) template.
        GroundTermPattern::Triple(tp) => {
            let s = ground_term(&tp.subject, sol)?;
            let p = named_term(&tp.predicate, sol)?;
            let o = ground_term(&tp.object, sol)?;
            Some(format!("<<{s} {p} {o}>>"))
        }
    }
}

/// Resolve an RDF-star quoted triple in a CONSTRUCT/INSERT template against a
/// solution → the canonical `<< s p o >>` token. Recurses for nesting; a blank
/// node inside a quoted triple yields `None` (not minted).
fn quoted_term_token(t: &TermPattern, sol: &rete_core::Binding) -> Option<String> {
    match t {
        TermPattern::NamedNode(n) => Some(n.to_string()),
        TermPattern::Literal(l) => Some(l.to_string()),
        TermPattern::Variable(v) => sol.get(v.as_str()).cloned(),
        TermPattern::BlankNode(_) => None,
        TermPattern::Triple(tp) => {
            let s = quoted_term_token(&tp.subject, sol)?;
            let p = named_term(&tp.predicate, sol)?;
            let o = quoted_term_token(&tp.object, sol)?;
            Some(format!("<<{s} {p} {o}>>"))
        }
    }
}

fn named_term(t: &NamedNodePattern, sol: &rete_core::Binding) -> Option<String> {
    match t {
        NamedNodePattern::NamedNode(n) => Some(n.to_string()),
        NamedNodePattern::Variable(v) => sol.get(v.as_str()).cloned(),
    }
}

// --- HTTP ----------------------------------------------------------------------

/// Percent-decode one `application/x-www-form-urlencoded` value (`+` = space).
fn form_unescape(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Split a query string / form body into decoded `(key, value)` pairs.
fn form_pairs(s: &str) -> Vec<(String, String)> {
    s.split('&')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((form_unescape(k), form_unescape(v)))
        })
        .collect()
}

fn param(pairs: &[(String, String)], key: &str) -> Option<String> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

/// What one HTTP request asks for, normalized across the SPARQL Protocol's
/// transports (GET query param, form body, raw `application/sparql-*` body).
enum Action {
    Query(String),
    Update(String),
    Snapshot,
    Info,
    Preflight,
    NotFound,
}

fn classify(req: &mut tiny_http::Request) -> anyhow::Result<Action> {
    use tiny_http::Method;
    let url = req.url().to_string();
    let (path, query_string) = match url.split_once('?') {
        Some((p, q)) => (p, Some(q.to_string())),
        None => (url.as_str(), None),
    };
    let path = path.trim_end_matches('/');
    let content_type = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Type"))
        .map(|h| h.value.as_str().to_ascii_lowercase())
        .unwrap_or_default();

    Ok(match (req.method().clone(), path) {
        (Method::Options, _) => Action::Preflight,
        (Method::Get, "" | "/") => Action::Info,
        (Method::Get, "/snapshot.rete") => Action::Snapshot,
        (Method::Get, "/sparql") => match query_string.as_deref().map(form_pairs) {
            Some(pairs) => match param(&pairs, "query") {
                Some(q) => Action::Query(q),
                None => Action::Info, // a bare GET /sparql: describe the service
            },
            None => Action::Info,
        },
        (Method::Post, "/sparql" | "/update") => {
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            if content_type.starts_with("application/sparql-query") {
                Action::Query(body)
            } else if content_type.starts_with("application/sparql-update") {
                Action::Update(body)
            } else {
                // Form-encoded (the protocol's default POST transport).
                let pairs = form_pairs(&body);
                if let Some(u) = param(&pairs, "update") {
                    Action::Update(u)
                } else if let Some(q) = param(&pairs, "query") {
                    Action::Query(q)
                } else {
                    anyhow::bail!("POST body carries neither `query` nor `update`");
                }
            }
        }
        _ => Action::NotFound,
    })
}

/// Build a response with the CORS headers every browser client needs.
fn respond(
    req: tiny_http::Request,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
) -> anyhow::Result<()> {
    let mut resp = tiny_http::Response::from_data(body).with_status_code(status);
    for (k, v) in [
        ("Content-Type", content_type),
        ("Access-Control-Allow-Origin", "*"),
        ("Access-Control-Allow-Methods", "GET, POST, OPTIONS"),
        (
            "Access-Control-Allow-Headers",
            "Content-Type, Authorization",
        ),
    ] {
        resp.add_header(
            tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes())
                .map_err(|_| anyhow::anyhow!("bad header"))?,
        );
    }
    req.respond(resp)?;
    Ok(())
}

/// `rete serve`: run the endpoint until interrupted.
pub(crate) fn serve(
    file: &str,
    bind: &str,
    token: Option<&str>,
    journal: Option<&str>,
) -> anyhow::Result<()> {
    let journal_path = std::path::PathBuf::from(
        journal
            .map(String::from)
            .unwrap_or_else(|| format!("{file}.changes")),
    );
    let mut store = Store::open(file, journal_path.clone())?;
    let server = tiny_http::Server::http(bind).map_err(|e| anyhow::anyhow!("bind {bind}: {e}"))?;
    eprintln!(
        "serving {file} ({} quads) at http://{bind}/sparql\n  updates: POST query=/update= (SPARQL 1.1 Protocol){}\n  journal: {}\n  snapshot: GET /snapshot.rete",
        store.quads.len(),
        if token.is_some() { " — Bearer token required" } else { "" },
        journal_path.display(),
    );

    for mut req in server.incoming_requests() {
        let action = match classify(&mut req) {
            Ok(a) => a,
            Err(e) => {
                let _ = respond(
                    req,
                    400,
                    "text/plain",
                    format!("bad request: {e}\n").into_bytes(),
                );
                continue;
            }
        };
        let result = match action {
            Action::Preflight => respond(req, 204, "text/plain", Vec::new()),
            Action::Info => {
                let body = format!(
                    "rete SPARQL endpoint\n  {} quads ({} pending journal file: {})\n  GET/POST /sparql   query= (SPARQL 1.1 Protocol; results JSON)\n  POST /sparql|/update  update= or application/sparql-update\n  GET /snapshot.rete    the current state as a .rete file\n",
                    store.quads.len(),
                    if store.dirty { "dirty;" } else { "clean;" },
                    store.journal.display(),
                );
                respond(req, 200, "text/plain; charset=utf-8", body.into_bytes())
            }
            Action::Snapshot => match store.ensure_current() {
                Ok(()) => respond(req, 200, "application/octet-stream", store.image.clone()),
                Err(e) => respond(req, 500, "text/plain", format!("{e}\n").into_bytes()),
            },
            Action::Query(q) => match store.ensure_current() {
                Ok(()) => match eval_query(&store.rete, &q) {
                    Ok(QueryOutput::Select(vars, rows)) => {
                        eprintln!("  query: {} row(s)", rows.len());
                        respond(
                            req,
                            200,
                            "application/sparql-results+json",
                            sparql_json_results(&vars, &rows).into_bytes(),
                        )
                    }
                    Ok(QueryOutput::Ask(b)) => respond(
                        req,
                        200,
                        "application/sparql-results+json",
                        sparql_json_ask(b).into_bytes(),
                    ),
                    Ok(QueryOutput::Construct(triples)) => {
                        let mut body = String::new();
                        for (s, p, o) in &triples {
                            body.push_str(&format!("{s} {p} {o} .\n"));
                        }
                        respond(req, 200, "application/n-triples", body.into_bytes())
                    }
                    Ok(_) => respond(
                        req,
                        501,
                        "text/plain",
                        b"query result kind is not supported by this server build\n".to_vec(),
                    ),
                    Err(e) => respond(req, 400, "text/plain", format!("{e}\n").into_bytes()),
                },
                Err(e) => respond(req, 500, "text/plain", format!("{e}\n").into_bytes()),
            },
            Action::Update(u) => {
                // Updates may be token-guarded; queries stay open.
                let authorized = match token {
                    None => true,
                    Some(t) => req
                        .headers()
                        .iter()
                        .find(|h| h.field.equiv("Authorization"))
                        .map(|h| h.value.as_str() == format!("Bearer {t}"))
                        .unwrap_or(false),
                };
                if !authorized {
                    respond(
                        req,
                        401,
                        "text/plain",
                        b"update requires Bearer token\n".to_vec(),
                    )
                } else {
                    match store.update(&u) {
                        Ok(n) => {
                            eprintln!("  update: {n} change(s)");
                            respond(req, 204, "text/plain", Vec::new())
                        }
                        Err(e) => respond(req, 400, "text/plain", format!("{e}\n").into_bytes()),
                    }
                }
            }
            Action::NotFound => respond(req, 404, "text/plain", b"not found\n".to_vec()),
        };
        if let Err(e) = result {
            eprintln!("  response error: {e}");
        }
    }
    Ok(())
}
