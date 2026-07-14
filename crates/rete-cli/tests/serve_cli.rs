//! `rete serve` end-to-end over real HTTP: SPARQL Protocol queries, SPARQL
//! Update mutating the served state, journal durability across a restart, the
//! `/snapshot.rete` companion, and the update token guard.

use std::io::Read as _;
use std::process::{Child, Command};

// `free_port` necessarily releases its probe socket before the child binds it.
// Serialize these two real-server tests so parallel test threads cannot both
// select the same just-released port and then talk to the wrong server.
static SERVER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn temp_path(name: &str, ext: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "rete_serve_cli_{}_{}.{}",
        std::process::id(),
        name,
        ext
    ))
}

/// A free loopback port (bind :0, read the assigned port, release it).
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Spawn `rete serve` and wait until it answers.
// The returned server is killed + waited by every test's tail (and by the
// timeout path below), which clippy's zombie analysis can't see across fns.
#[allow(clippy::zombie_processes)]
fn spawn_server(file: &std::path::Path, port: u16, extra: &[&str]) -> Child {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rete"));
    cmd.arg("serve")
        .arg(file)
        .arg("--bind")
        .arg(format!("127.0.0.1:{port}"))
        .args(extra)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().expect("spawn rete serve");
    for _ in 0..100 {
        if ureq::get(&format!("http://127.0.0.1:{port}/"))
            .call()
            .is_ok()
        {
            return child;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("server did not come up on port {port}");
}

fn stop_server(mut child: Child) {
    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .expect("send SIGINT to rete serve");
        assert!(status.success());
    }
    #[cfg(not(unix))]
    child.kill().expect("stop rete serve");
    let status = child.wait().expect("wait for rete serve");
    assert!(status.success(), "rete serve did not shut down cleanly");
}

fn select_rows(port: u16, query: &str) -> Vec<rete_core::Binding> {
    let body = ureq::post(&format!("http://127.0.0.1:{port}/sparql"))
        .send_form(&[("query", query)])
        .expect("query ok")
        .into_string()
        .unwrap();
    rete_core::parse_sparql_json_results(&body).expect("valid results JSON")
}

fn post_update(port: u16, update: &str) -> Result<u16, u16> {
    match ureq::post(&format!("http://127.0.0.1:{port}/sparql")).send_form(&[("update", update)]) {
        Ok(r) => Ok(r.status()),
        Err(ureq::Error::Status(code, _)) => Err(code),
        Err(e) => panic!("transport: {e}"),
    }
}

fn build_fixture(name: &str) -> std::path::PathBuf {
    let nt = temp_path(name, "nt");
    let file = temp_path(name, "rete");
    std::fs::write(
        &nt,
        "<http://ex/a> <http://ex/name> \"Alice\" .\n\
         <http://ex/b> <http://ex/name> \"Bob\" .\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rete"))
        .args([
            "build",
            nt.to_str().unwrap(),
            "-o",
            file.to_str().unwrap(),
            "--no-pyramid",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "build: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // A stale journal from a previous test run must not leak in.
    let _ = std::fs::remove_file(format!("{}.changes", file.display()));
    file
}

#[test]
fn serve_query_update_snapshot_and_journal_replay() {
    let _server_test_guard = SERVER_TEST_LOCK.lock().unwrap();
    let file = build_fixture("replay");
    let port = free_port();
    let server = spawn_server(&file, port, &[]);

    let all = "SELECT ?s ?n WHERE { ?s <http://ex/name> ?n } ORDER BY ?n";
    assert_eq!(select_rows(port, all).len(), 2);

    // INSERT DATA becomes visible to the very next query.
    assert_eq!(
        post_update(
            port,
            "INSERT DATA { <http://ex/c> <http://ex/name> \"Carol\" }"
        ),
        Ok(204)
    );
    let rows = select_rows(port, all);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().any(|b| b["n"] == "\"Carol\""));

    // DELETE/INSERT WHERE: rename Bob (pattern evaluated on current state).
    assert_eq!(
        post_update(
            port,
            "DELETE { ?s <http://ex/name> \"Bob\" } INSERT { ?s <http://ex/name> \"Robert\" } \
             WHERE { ?s <http://ex/name> \"Bob\" }"
        ),
        Ok(204)
    );
    let rows = select_rows(port, all);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().any(|b| b["n"] == "\"Robert\""));
    assert!(!rows.iter().any(|b| b["n"] == "\"Bob\""));

    // DELETE DATA removes exactly one row.
    assert_eq!(
        post_update(
            port,
            "DELETE DATA { <http://ex/a> <http://ex/name> \"Alice\" }"
        ),
        Ok(204)
    );
    assert_eq!(select_rows(port, all).len(), 2);

    // The snapshot is a valid .rete of the CURRENT state (base never mutated).
    let mut snapshot = Vec::new();
    ureq::get(&format!("http://127.0.0.1:{port}/snapshot.rete"))
        .call()
        .unwrap()
        .into_reader()
        .read_to_end(&mut snapshot)
        .unwrap();
    let snap = rete_core::Rete::open(&snapshot).expect("snapshot opens");
    let (_, rows) = rete_core::eval_sparql(&snap, all).unwrap();
    assert_eq!(rows.len(), 2, "snapshot carries the updated state");
    let base = rete_core::Rete::open(&std::fs::read(&file).unwrap()).unwrap();
    let (_, rows) = rete_core::eval_sparql(&base, all).unwrap();
    assert_eq!(rows.len(), 2, "base file untouched (still Alice+Bob)");
    assert!(
        rows.iter().any(|b| b["n"] == "\"Alice\""),
        "base still has Alice — updates went to the journal only"
    );

    // Restart: the journal replays and the updated state survives.
    stop_server(server);
    let port2 = free_port();
    let server2 = spawn_server(&file, port2, &[]);
    let rows = select_rows(port2, all);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|b| b["n"] == "\"Robert\""));
    assert!(!rows.iter().any(|b| b["n"] == "\"Alice\""));
    stop_server(server2);

    let _ = std::fs::remove_file(format!("{}.changes", file.display()));
}

#[test]
fn serve_update_token_guards_writes_not_reads() {
    let _server_test_guard = SERVER_TEST_LOCK.lock().unwrap();
    let file = build_fixture("guarded");
    // Use a dedicated journal so the two tests never share state.
    let journal = temp_path("guarded", "changes");
    let _ = std::fs::remove_file(&journal);
    let port = free_port();
    let server = spawn_server(
        &file,
        port,
        &["--token", "s3cret", "--journal", journal.to_str().unwrap()],
    );

    // Reads stay open.
    let all = "SELECT ?s WHERE { ?s <http://ex/name> ?n }";
    assert_eq!(select_rows(port, all).len(), 2);

    // Unauthenticated update → 401 and no change.
    assert_eq!(
        post_update(port, "INSERT DATA { <http://ex/x> <http://ex/name> \"X\" }"),
        Err(401)
    );
    assert_eq!(select_rows(port, all).len(), 2);

    // With the Bearer token → applied.
    let status = ureq::post(&format!("http://127.0.0.1:{port}/sparql"))
        .set("Authorization", "Bearer s3cret")
        .send_form(&[(
            "update",
            "INSERT DATA { <http://ex/x> <http://ex/name> \"X\" }",
        )])
        .unwrap()
        .status();
    assert_eq!(status, 204);
    assert_eq!(select_rows(port, all).len(), 3);

    stop_server(server);
    let _ = std::fs::remove_file(&journal);
}
