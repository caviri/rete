mod common;

#[test]
fn generate_writes_all_release_shell_and_man_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    common::rete()
        .args(["generate", "--output"])
        .arg(dir.path())
        .assert()
        .success();

    for name in ["rete.bash", "_rete", "rete.fish", "rete.ps1", "rete.1"] {
        let path = dir.path().join(name);
        let contents =
            std::fs::read(&path).unwrap_or_else(|error| panic!("missing {name}: {error}"));
        assert!(contents.len() > 100, "{name} is unexpectedly small");
    }

    let man = std::fs::read_to_string(dir.path().join("rete.1")).unwrap();
    assert!(man.contains(&format!("rete {}", env!("CARGO_PKG_VERSION"))));
}
