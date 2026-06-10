//! LUBM-style benchmark: the Lehigh University Benchmark's data model and its
//! 14 standard queries, run cross-engine (rete vs Oxigraph) on identical,
//! pre-materialized data.
//!
//! **Honesty notes.** The data generator is a faithful *reimplementation* of
//! UBA's documented cardinalities (universities → departments → faculty /
//! courses / publications / students), not the official Java tool, so absolute
//! counts differ from published LUBM numbers; the correctness anchor here is
//! **cross-engine row parity** on the same triples. Queries that depend on
//! OWL restriction classes (`Student ≡ ∃takesCourse`, `Chair ≡ ∃headOf`) are
//! answered under RDFS-level materialization only — `rete reason`'s subset
//! (subclass/subproperty/inverse/transitive) is applied up front and **both**
//! engines query the same materialized graph, so Q6/Q10 cover explicit
//! `UndergraduateStudent`s and Q12 is empty on both sides, by construction.

use std::io::BufReader;
use std::time::Instant;

use anyhow::{ensure, Context, Result};
use oxigraph::io::RdfFormat;
use oxigraph::store::Store;
use rete_core::{reason, DictionaryBuilder, GraphIndexBuilder, Rete};
use serde_json::json;

use crate::{bench, mem, oxi_try, pm, rete_try};

const UB: &str = "http://swat.cse.lehigh.edu/onto/univ-bench.owl#";
const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

type Triple = (String, String, String);

/// Deterministic xorshift RNG (the generator must be reproducible).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in `lo..=hi`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

fn ub(name: &str) -> String {
    format!("<{UB}{name}>")
}

fn univ(u: u64) -> String {
    format!("<http://www.University{u}.edu>")
}

fn dept(u: u64, d: u64) -> String {
    format!("<http://www.Department{d}.University{u}.edu>")
}

fn entity(u: u64, d: u64, name: &str) -> String {
    format!("<http://www.Department{d}.University{u}.edu/{name}>")
}

/// The univ-bench ontology subset that rete's RDFS/OWL-RL reasoner can
/// materialize (class/property hierarchies, the inverse alumnus link, and the
/// transitive sub-organization property).
fn ontology() -> Vec<Triple> {
    let sub_class = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
    let sub_prop = "<http://www.w3.org/2000/01/rdf-schema#subPropertyOf>";
    let inverse = "<http://www.w3.org/2002/07/owl#inverseOf>";
    let transitive = "<http://www.w3.org/2002/07/owl#TransitiveProperty>";
    let mut t = Vec::new();
    let mut sc = |a: &str, b: &str| t.push((ub(a), sub_class.to_string(), ub(b)));
    sc("FullProfessor", "Professor");
    sc("AssociateProfessor", "Professor");
    sc("AssistantProfessor", "Professor");
    sc("Professor", "Faculty");
    sc("Lecturer", "Faculty");
    sc("Faculty", "Employee");
    sc("Employee", "Person");
    sc("UndergraduateStudent", "Student");
    sc("Student", "Person");
    sc("GraduateStudent", "Person");
    sc("TeachingAssistant", "Person");
    sc("ResearchAssistant", "Person");
    sc("GraduateCourse", "Course");
    let mut sp = |a: &str, b: &str| t.push((ub(a), sub_prop.to_string(), ub(b)));
    sp("worksFor", "memberOf");
    sp("headOf", "worksFor");
    sp("undergraduateDegreeFrom", "degreeFrom");
    sp("mastersDegreeFrom", "degreeFrom");
    sp("doctoralDegreeFrom", "degreeFrom");
    t.push((ub("hasAlumnus"), inverse.to_string(), ub("degreeFrom")));
    t.push((
        ub("subOrganizationOf"),
        RDF_TYPE.to_string(),
        transitive.to_string(),
    ));
    t
}

fn push3(t: &mut Vec<Triple>, s: &str, p: String, o: &str) {
    t.push((s.to_string(), p, o.to_string()));
}

/// Generate `universities` LUBM-style universities with UBA's documented
/// cardinalities and URI scheme.
fn generate(universities: u64) -> Vec<Triple> {
    let mut rng = Rng(0x1BAD_5EED ^ (universities << 32 | 0x10BA));
    let mut t: Vec<Triple> = ontology();

    for u in 0..universities {
        let university = univ(u);
        push3(&mut t, &university, RDF_TYPE.into(), &ub("University"));
        let depts = rng.range(15, 25);
        for d in 0..depts {
            let department = dept(u, d);
            push3(&mut t, &department, RDF_TYPE.into(), &ub("Department"));
            push3(&mut t, &department, ub("subOrganizationOf"), &university);

            // Faculty by rank, with degrees, contact data, courses, papers.
            let ranks: [(&str, u64, u64, u64, u64); 4] = [
                ("FullProfessor", rng.range(7, 10), 15, 20, 0),
                ("AssociateProfessor", rng.range(10, 14), 10, 18, 0),
                ("AssistantProfessor", rng.range(8, 11), 5, 10, 0),
                ("Lecturer", rng.range(5, 7), 0, 5, 1),
            ];
            let mut faculty: Vec<String> = Vec::new();
            let mut professors: Vec<String> = Vec::new();
            let mut courses: Vec<String> = Vec::new();
            let mut grad_courses: Vec<String> = Vec::new();
            let mut course_seq = 0u64;
            let mut grad_course_seq = 0u64;
            for (rank, count, pub_lo, pub_hi, is_lecturer) in ranks {
                for i in 0..count {
                    let person = entity(u, d, &format!("{rank}{i}"));
                    push3(&mut t, &person, RDF_TYPE.into(), &ub(rank));
                    push3(&mut t, &person, ub("worksFor"), &department);
                    t.push((person.clone(), ub("name"), format!("\"{rank}{i}\"")));
                    t.push((
                        person.clone(),
                        ub("emailAddress"),
                        format!("\"{rank}{i}@Department{d}.University{u}.edu\""),
                    ));
                    t.push((
                        person.clone(),
                        ub("telephone"),
                        format!("\"xxx-xxx-{:04}\"", rng.range(0, 9999)),
                    ));
                    for degree in [
                        "undergraduateDegreeFrom",
                        "mastersDegreeFrom",
                        "doctoralDegreeFrom",
                    ] {
                        push3(
                            &mut t,
                            &person,
                            ub(degree),
                            &univ(rng.range(0, universities - 1)),
                        );
                    }
                    // Courses taught (1-2 undergrad + 1-2 graduate each).
                    for _ in 0..rng.range(1, 2) {
                        let c = entity(u, d, &format!("Course{course_seq}"));
                        course_seq += 1;
                        push3(&mut t, &c, RDF_TYPE.into(), &ub("Course"));
                        push3(&mut t, &person, ub("teacherOf"), &c);
                        courses.push(c);
                    }
                    for _ in 0..rng.range(1, 2) {
                        let c = entity(u, d, &format!("GraduateCourse{grad_course_seq}"));
                        grad_course_seq += 1;
                        push3(&mut t, &c, RDF_TYPE.into(), &ub("GraduateCourse"));
                        push3(&mut t, &person, ub("teacherOf"), &c);
                        grad_courses.push(c);
                    }
                    // Publications.
                    for j in 0..rng.range(pub_lo, pub_hi.max(pub_lo)) {
                        let p = entity(u, d, &format!("{rank}{i}/Publication{j}"));
                        push3(&mut t, &p, RDF_TYPE.into(), &ub("Publication"));
                        push3(&mut t, &p, ub("publicationAuthor"), &person);
                    }
                    if is_lecturer == 0 {
                        professors.push(person.clone());
                    }
                    faculty.push(person);
                }
            }
            // One full professor heads the department.
            push3(&mut t, &faculty[0], ub("headOf"), &department);

            // Research groups.
            for g in 0..rng.range(10, 20) {
                let group = entity(u, d, &format!("ResearchGroup{g}"));
                push3(&mut t, &group, RDF_TYPE.into(), &ub("ResearchGroup"));
                push3(&mut t, &group, ub("subOrganizationOf"), &department);
            }

            // Students: 8-14 undergrads and 3-4 grads per faculty member.
            let undergrads = faculty.len() as u64 * rng.range(8, 14);
            let grads = faculty.len() as u64 * rng.range(3, 4);
            for i in 0..undergrads {
                let s = entity(u, d, &format!("UndergraduateStudent{i}"));
                push3(&mut t, &s, RDF_TYPE.into(), &ub("UndergraduateStudent"));
                push3(&mut t, &s, ub("memberOf"), &department);
                t.push((
                    s.clone(),
                    ub("name"),
                    format!("\"UndergraduateStudent{i}\""),
                ));
                t.push((
                    s.clone(),
                    ub("emailAddress"),
                    format!("\"UndergraduateStudent{i}@Department{d}.University{u}.edu\""),
                ));
                for _ in 0..rng.range(2, 4) {
                    let c = &courses[(rng.next() as usize) % courses.len()];
                    push3(&mut t, &s, ub("takesCourse"), c);
                }
                if rng.range(1, 5) == 1 {
                    let a = &professors[(rng.next() as usize) % professors.len()];
                    push3(&mut t, &s, ub("advisor"), a);
                }
            }
            for i in 0..grads {
                let s = entity(u, d, &format!("GraduateStudent{i}"));
                push3(&mut t, &s, RDF_TYPE.into(), &ub("GraduateStudent"));
                push3(&mut t, &s, ub("memberOf"), &department);
                t.push((s.clone(), ub("name"), format!("\"GraduateStudent{i}\"")));
                t.push((
                    s.clone(),
                    ub("emailAddress"),
                    format!("\"GraduateStudent{i}@Department{d}.University{u}.edu\""),
                ));
                push3(
                    &mut t,
                    &s,
                    ub("undergraduateDegreeFrom"),
                    &univ(rng.range(0, universities - 1)),
                );
                for _ in 0..rng.range(1, 3) {
                    let c = &grad_courses[(rng.next() as usize) % grad_courses.len()];
                    push3(&mut t, &s, ub("takesCourse"), c);
                }
                let a = &professors[(rng.next() as usize) % professors.len()];
                push3(&mut t, &s, ub("advisor"), a);
                if rng.range(1, 5) == 1 {
                    push3(&mut t, &s, RDF_TYPE.into(), &ub("TeachingAssistant"));
                    let c = &courses[(rng.next() as usize) % courses.len()];
                    push3(&mut t, &s, ub("teachingAssistantOf"), c);
                }
                if rng.range(1, 4) == 1 {
                    push3(&mut t, &s, RDF_TYPE.into(), &ub("ResearchAssistant"));
                }
            }
        }
    }
    t
}

/// The 14 standard LUBM queries (their published SPARQL forms), parameterized
/// on University0 / Department0 per the benchmark definition.
fn queries() -> Vec<(&'static str, String)> {
    let prefix =
        format!("PREFIX ub: <{UB}> PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> ");
    let d0 = dept(0, 0);
    let u0 = univ(0);
    let gc0 = entity(0, 0, "GraduateCourse0");
    let ap0 = entity(0, 0, "AssociateProfessor0");
    let asst0 = entity(0, 0, "AssistantProfessor0");
    let q = |body: String| format!("{prefix}{body}");
    vec![
        ("Q1 grad students of a course",
         q(format!("SELECT ?x WHERE {{ ?x rdf:type ub:GraduateStudent . ?x ub:takesCourse {gc0} }}"))),
        ("Q2 grad student / univ / dept triangle",
         q("SELECT ?x ?y ?z WHERE { ?x rdf:type ub:GraduateStudent . ?y rdf:type ub:University . ?z rdf:type ub:Department . ?x ub:memberOf ?z . ?z ub:subOrganizationOf ?y . ?x ub:undergraduateDegreeFrom ?y }".into())),
        ("Q3 publications of a professor",
         q(format!("SELECT ?x WHERE {{ ?x rdf:type ub:Publication . ?x ub:publicationAuthor {asst0} }}"))),
        ("Q4 professors of a department (+3 props)",
         q(format!("SELECT ?x ?y1 ?y2 ?y3 WHERE {{ ?x rdf:type ub:Professor . ?x ub:worksFor {d0} . ?x ub:name ?y1 . ?x ub:emailAddress ?y2 . ?x ub:telephone ?y3 }}"))),
        ("Q5 persons member of a department",
         q(format!("SELECT ?x WHERE {{ ?x rdf:type ub:Person . ?x ub:memberOf {d0} }}"))),
        ("Q6 all students",
         q("SELECT ?x WHERE { ?x rdf:type ub:Student }".into())),
        ("Q7 students of a professor's courses",
         q(format!("SELECT ?x ?y WHERE {{ ?x rdf:type ub:Student . ?y rdf:type ub:Course . ?x ub:takesCourse ?y . {ap0} ub:teacherOf ?y }}"))),
        ("Q8 students of a university's departments",
         q(format!("SELECT ?x ?y ?z WHERE {{ ?x rdf:type ub:Student . ?y rdf:type ub:Department . ?x ub:memberOf ?y . ?y ub:subOrganizationOf {u0} . ?x ub:emailAddress ?z }}"))),
        ("Q9 student / advisor / course triangle",
         q("SELECT ?x ?y ?z WHERE { ?x rdf:type ub:Student . ?y rdf:type ub:Faculty . ?z rdf:type ub:Course . ?x ub:advisor ?y . ?y ub:teacherOf ?z . ?x ub:takesCourse ?z }".into())),
        ("Q10 students of a graduate course",
         q(format!("SELECT ?x WHERE {{ ?x rdf:type ub:Student . ?x ub:takesCourse {gc0} }}"))),
        ("Q11 research groups of a university",
         q(format!("SELECT ?x WHERE {{ ?x rdf:type ub:ResearchGroup . ?x ub:subOrganizationOf {u0} }}"))),
        ("Q12 chairs of a university's departments",
         q(format!("SELECT ?x ?y WHERE {{ ?x rdf:type ub:Chair . ?y rdf:type ub:Department . ?x ub:worksFor ?y . ?y ub:subOrganizationOf {u0} }}"))),
        ("Q13 alumni of a university",
         q(format!("SELECT ?x WHERE {{ ?x rdf:type ub:Person . {u0} ub:hasAlumnus ?x }}"))),
        ("Q14 all undergraduate students",
         q("SELECT ?x WHERE { ?x rdf:type ub:UndergraduateStudent }".into())),
    ]
}

pub fn run(json_out: bool, universities: usize) -> Result<()> {
    let reps = 5;
    let universities = universities.max(1) as u64;

    // ---- Generate + materialize (RDFS/OWL-RL subset) ----
    let t = Instant::now();
    let base = generate(universities);
    let gen_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let reasoning = reason(&base);
    let reason_ms = t.elapsed().as_secs_f64() * 1000.0;
    ensure!(
        reasoning.inconsistencies.is_empty(),
        "LUBM ontology materialization reported inconsistencies"
    );
    let mut all = base.clone();
    all.extend(reasoning.inferred.iter().cloned());
    all.sort_unstable();
    all.dedup();

    // ---- Build both engines on the same materialized triples ----
    let mut db = DictionaryBuilder::new();
    for (s, p, o) in &all {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let mut ib = GraphIndexBuilder::new();
    for (s, p, o) in &all {
        ib.push(dict.encode(s, p, o).context("encode")?);
    }
    let t = Instant::now();
    let bytes = rete_core::write_file(&dict, &ib.build(), false, &[], 0);
    let build_ms = t.elapsed().as_secs_f64() * 1000.0;
    let heap0 = mem::live();
    let t = Instant::now();
    let rete = Rete::open(&bytes).context("Rete::open")?;
    let rete_open_ms = t.elapsed().as_secs_f64() * 1000.0;
    let rete_heap = mem::live().saturating_sub(heap0);

    let mut nt = String::new();
    for (s, p, o) in &all {
        nt.push_str(&format!("{s} {p} {o} .\n"));
    }
    let heap1 = mem::live();
    let store = Store::new().context("Store::new")?;
    let t = Instant::now();
    store
        .load_from_reader(RdfFormat::NTriples, BufReader::new(nt.as_bytes()))
        .context("oxigraph load")?;
    let oxi_load_ms = t.elapsed().as_secs_f64() * 1000.0;
    let oxi_heap = mem::live().saturating_sub(heap1);

    if !json_out {
        println!("# LUBM-style benchmark — rete vs Oxigraph\n");
        println!(
            "LUBM({universities}) reimplementation: {} base + {} materialized = {} triples \
             (gen {gen_ms:.0} ms · RDFS/OWL-RL materialization {reason_ms:.0} ms · \
             `.rete` build {build_ms:.0} ms, {} bytes). Cross-engine row parity is the \
             correctness anchor; counts are not comparable to published LUBM numbers \
             (reimplemented generator, RDFS-level inference only — Q12's OWL-defined \
             `Chair` class is empty on both engines by construction).\n",
            base.len(),
            reasoning.inferred.len(),
            all.len(),
            bytes.len()
        );
        println!("| Engine | Load | Resident heap |");
        println!("|---|--:|--:|");
        println!(
            "| rete `Rete::open` | {rete_open_ms:.1} ms | {} MiB |",
            mem::mib(rete_heap)
        );
        println!(
            "| Oxigraph bulk-load | {oxi_load_ms:.0} ms | {} MiB |",
            mem::mib(oxi_heap)
        );
        println!();
        println!("| Query | rete (ms) | Oxigraph (ms) | rete vs oxi | peak heap MiB (rete / oxi) | rows | ✓ |");
        println!("|---|--:|--:|--:|--:|--:|:--:|");
    }

    let mut rows_json = Vec::new();
    let mut agree = 0;
    let mut mismatches: Vec<String> = Vec::new();
    let qs = queries();
    for (name, q) in &qs {
        let rr = rete_try(&rete, q).map_err(|e| anyhow::anyhow!("{name}: rete: {e}"))?;
        let or = oxi_try(&store, q).map_err(|e| anyhow::anyhow!("{name}: oxigraph: {e}"))?;
        let ok = rr == or;
        if ok {
            agree += 1;
        } else {
            mismatches.push(format!("{name}: rete {rr} vs oxigraph {or}"));
        }
        let (rete_m, _) = bench(reps, || rete_try(&rete, q).unwrap_or(0));
        let (oxi_m, _) = bench(reps, || oxi_try(&store, q).unwrap_or(0));
        let speedup = oxi_m.median_ms / rete_m.median_ms;
        rows_json.push(json!({
            "name": name,
            "query": q,
            "rete_ms": rete_m.median_ms,
            "rete_ms_sd": rete_m.sd_ms,
            "rete_peak_heap_bytes": rete_m.peak_heap,
            "oxigraph_ms": oxi_m.median_ms,
            "oxigraph_ms_sd": oxi_m.sd_ms,
            "oxigraph_peak_heap_bytes": oxi_m.peak_heap,
            "speedup": speedup,
            "rows": rr,
            "agree": ok,
        }));
        if !json_out {
            println!(
                "| {name} | {} | {} | {speedup:.1}× | {} / {} | {rr} | {} |",
                pm(&rete_m),
                pm(&oxi_m),
                mem::mib(rete_m.peak_heap),
                mem::mib(oxi_m.peak_heap),
                if ok { "✓" } else { "✗" }
            );
        }
    }
    if !json_out {
        println!(
            "\n{agree}/{} queries returned identical row counts on both engines.",
            qs.len()
        );
        if let Some(kb) = mem::vm_hwm_kb() {
            println!("Process peak RSS (`VmHWM`): {:.1} MiB.", kb as f64 / 1024.0);
        }
    } else {
        let report = json!({
            "schema_version": 1,
            "tool": "rete-bench-lubm",
            "universities": universities,
            "base_triples": base.len(),
            "materialized_triples": reasoning.inferred.len(),
            "total_triples": all.len(),
            "rete_bytes": bytes.len(),
            "generate_ms": gen_ms,
            "reason_ms": reason_ms,
            "build_ms": build_ms,
            "load_open": {
                "rete_ms": rete_open_ms,
                "oxigraph_ms": oxi_load_ms,
                "rete_heap_bytes": rete_heap,
                "oxigraph_heap_bytes": oxi_heap,
                "process_peak_rss_kb": mem::vm_hwm_kb(),
            },
            "queries": rows_json,
            "query_agreement": { "agree": agree, "total": qs.len() },
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    ensure!(
        mismatches.is_empty(),
        "cross-engine LUBM mismatch:\n  {}",
        mismatches.join("\n  ")
    );
    Ok(())
}
