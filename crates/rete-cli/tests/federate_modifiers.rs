//! `rete federate` applies the query's solution modifiers to the MERGED rows:
//! each source answers on its own, so ORDER BY, OFFSET, LIMIT and aggregates
//! used to come back per source and concatenated (G42). Two small files here;
//! `a` and `b` share no triple.

mod common;

use predicates::prelude::*;

const A: &str = "\
<http://p/zeta> <http://e/downloads> \"5\"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://p/zeta> <http://e/kind> \"lib\" .
<http://p/alpha> <http://e/downloads> \"10\"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://p/alpha> <http://e/kind> \"app\" .
<http://p/mu> <http://e/downloads> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://p/mu> <http://e/kind> \"lib\" .
";

const B: &str = "\
<http://p/beta> <http://e/downloads> \"7\"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://p/beta> <http://e/kind> \"lib\" .
<http://p/gamma> <http://e/downloads> \"3\"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://p/gamma> <http://e/kind> \"tool\" .
<http://p/delta> <http://e/downloads> \"8\"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://p/delta> <http://e/kind> \"app\" .
";

struct Shards {
    fx: common::Fixture,
    a: std::path::PathBuf,
    b: std::path::PathBuf,
}

fn shards() -> Shards {
    let fx = common::fixture();
    let mut rete = Vec::new();
    for (name, data) in [("a", A), ("b", B)] {
        let nt = fx.write(&format!("{name}.nt"), data);
        let out = fx.path(&format!("{name}.rete"));
        common::build(&nt, &out, &["--no-pyramid"]);
        rete.push(out);
    }
    let b = rete.pop().unwrap();
    let a = rete.pop().unwrap();
    Shards { fx, a, b }
}

impl Shards {
    fn cmd(&self, query: &str, extra: &[&str]) -> assert_cmd::Command {
        let mut c = common::rete();
        c.arg("federate")
            .arg(&self.a)
            .arg(&self.b)
            .args(["--query", query, "--json"])
            .args(extra);
        c
    }

    /// The rows as the values of `vars`, in order ("" for an unbound one).
    fn rows(&self, query: &str, extra: &[&str], vars: &[&str]) -> Vec<Vec<String>> {
        let _ = &self.fx;
        let v = common::json(&mut self.cmd(query, extra));
        v["results"]["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                vars.iter()
                    .map(|var| b[*var]["value"].as_str().unwrap_or("").to_string())
                    .collect()
            })
            .collect()
    }

    fn col(&self, query: &str, extra: &[&str], var: &str) -> Vec<String> {
        self.rows(query, extra, &[var])
            .into_iter()
            .map(|mut r| r.remove(0))
            .collect()
    }
}

const PKG: &str = "?package <http://e/downloads> ?d";

#[test]
fn order_by_sorts_the_merged_rows() {
    let s = shards();
    let q = format!("SELECT ?package WHERE {{ {PKG} }} ORDER BY ?package");
    let names: Vec<String> = s
        .col(&q, &[], "package")
        .iter()
        .map(|p| p.trim_start_matches("http://p/").to_string())
        .collect();
    assert_eq!(names, ["alpha", "beta", "delta", "gamma", "mu", "zeta"]);
    // Descending, and by a number's value (10 after 8, not before 3).
    let q = format!("SELECT ?package ?d WHERE {{ {PKG} }} ORDER BY DESC(?d)");
    let d = s.col(&q, &[], "d");
    assert_eq!(d, ["10", "8", "7", "5", "3", "1"]);
}

#[test]
fn offset_and_limit_cut_the_merged_rows_and_limit_goes_to_each_source() {
    let s = shards();
    let q = format!("SELECT ?d WHERE {{ {PKG} }} ORDER BY ?d OFFSET 1 LIMIT 3");
    assert_eq!(s.col(&q, &[], "d"), ["3", "5", "7"]);
    // Each source is asked for offset+limit = 4 rows at most, not all three
    // of its rows plus the cut afterwards: here that is 3 (it only has 3), so
    // use a smaller LIMIT to see the push-down in the per-source report.
    let q = format!("SELECT ?d WHERE {{ {PKG} }} ORDER BY DESC(?d) LIMIT 2");
    s.cmd(&q, &[])
        .assert()
        .success()
        .stderr(predicate::str::contains("2 row(s)").count(2));
    assert_eq!(s.col(&q, &[], "d"), ["10", "8"]);
    // Without a LIMIT nothing is pushed down, and OFFSET alone still applies.
    let q = format!("SELECT ?d WHERE {{ {PKG} }} ORDER BY ?d OFFSET 4");
    assert_eq!(s.col(&q, &[], "d"), ["8", "10"]);
}

#[test]
fn distinct_removes_rows_repeated_across_sources() {
    let s = shards();
    let q = "SELECT DISTINCT ?kind WHERE { ?p <http://e/kind> ?kind } ORDER BY ?kind";
    assert_eq!(s.col(q, &[], "kind"), ["app", "lib", "tool"]);
    let q = "SELECT DISTINCT ?kind WHERE { ?p <http://e/kind> ?kind } ORDER BY ?kind LIMIT 2";
    assert_eq!(s.col(q, &[], "kind"), ["app", "lib"]);
}

#[test]
fn count_and_sum_are_added_across_sources_when_they_are_disjoint() {
    let s = shards();
    let q = format!("SELECT (COUNT(*) AS ?n) (SUM(?d) AS ?t) WHERE {{ {PKG} }}");
    assert_eq!(s.rows(&q, &["--disjoint"], &["n", "t"]), [["6", "34"]]);
    // Not merged without the flag: it names the reason and the flag.
    s.cmd(&q, &[])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--disjoint"));
}

#[test]
fn min_and_max_merge_without_any_flag() {
    let s = shards();
    let q = format!("SELECT (MIN(?d) AS ?lo) (MAX(?d) AS ?hi) WHERE {{ {PKG} }}");
    assert_eq!(s.rows(&q, &[], &["lo", "hi"]), [["1", "10"]]);
}

#[test]
fn group_by_merges_groups_then_orders_and_limits() {
    let s = shards();
    let q = "SELECT ?kind (COUNT(?p) AS ?n) WHERE { ?p <http://e/kind> ?kind } \
             GROUP BY ?kind ORDER BY DESC(?n) ?kind";
    assert_eq!(
        s.rows(q, &["--disjoint"], &["kind", "n"]),
        [["lib", "3"], ["app", "2"], ["tool", "1"]]
    );
    let q = format!("{q} LIMIT 1");
    assert_eq!(s.rows(&q, &["--disjoint"], &["kind", "n"]), [["lib", "3"]]);
}

#[test]
fn what_cannot_merge_is_refused_with_the_reason() {
    let s = shards();
    for (q, why) in [
        (
            format!("SELECT (AVG(?d) AS ?a) WHERE {{ {PKG} }}"),
            "AVG cannot be merged",
        ),
        (
            "SELECT ?k (COUNT(*) AS ?n) WHERE { ?p <http://e/kind> ?k } GROUP BY ?k HAVING (COUNT(*) > 1)"
                .to_string(),
            "HAVING",
        ),
        (
            format!("SELECT ?package WHERE {{ {PKG} }} ORDER BY ?d"),
            "does not project",
        ),
        (
            format!("SELECT ?package WHERE {{ {PKG} }} ORDER BY STR(?package)"),
            "an expression",
        ),
    ] {
        s.cmd(&q, &["--disjoint"])
            .assert()
            .failure()
            .stderr(predicate::str::contains(why));
    }
}

#[test]
fn one_queried_source_answers_the_query_itself() {
    // Routing prunes `other` (it has no `e:downloads`): one source is queried,
    // and it answers the whole query, so an AVG is not refused.
    let fx = common::fixture();
    let nt = fx.write("only.nt", A);
    let a = fx.path("only.rete");
    common::build(&nt, &a, &[]);
    let other = fx.write("other.nt", "<http://p/x> <http://e/label> \"x\" .\n");
    let b = fx.path("other.rete");
    common::build(&other, &b, &[]);
    let v = common::json(common::rete().arg("federate").arg(&a).arg(&b).args([
        "--query",
        "SELECT (AVG(?d) AS ?a) WHERE { ?p <http://e/downloads> ?d }",
        "--json",
    ]));
    let avg = v["results"]["bindings"][0]["a"]["value"].as_str().unwrap();
    assert!(avg.starts_with("5.33"), "{avg}");
}
