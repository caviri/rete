#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::{io::Write as _, net::TcpListener};

use assert_cmd::Command;
use tempfile::TempDir;

pub const FIXTURE_NQ: &str = concat!(
    "<http://example.test/alice> <http://example.test/knows> <http://example.test/bob> .\n",
    "<http://example.test/bob> <http://example.test/name> \"Bob\"@en .\n",
    "<http://example.test/alice> <http://example.test/name> \"Alice\"@en <http://example.test/people> .\n",
);

pub struct Fixture {
    _dir: TempDir,
    pub source: PathBuf,
    pub rete: PathBuf,
}

impl Fixture {
    pub fn path(&self, name: &str) -> PathBuf {
        self._dir.path().join(name)
    }

    pub fn write(&self, name: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, contents).unwrap();
        path
    }
}

pub fn rete() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rete"))
}

pub fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("minimal.nq");
    let output = dir.path().join("minimal.rete");
    std::fs::write(&source, FIXTURE_NQ).unwrap();
    build(&source, &output, &["--no-pyramid"]);
    Fixture {
        _dir: dir,
        source,
        rete: output,
    }
}

pub fn build(source: &Path, output: &Path, extra: &[&str]) {
    let mut command = rete();
    command
        .arg("build")
        .arg(source)
        .arg("-o")
        .arg(output)
        .args(extra)
        .assert()
        .success();
}

pub fn json(command: &mut Command) -> serde_json::Value {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

#[derive(Clone, Copy)]
pub enum RangeMode {
    Honor,
    Ignore,
    NotFound,
}

pub fn serve(data: Vec<u8>, mode: RangeMode) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/graph.rete", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(stream) => stream,
                Err(_) => continue,
            };
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                use std::io::Read as _;
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                request.push(byte[0]);
            }
            if matches!(mode, RangeMode::NotFound) {
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                continue;
            }
            let text = String::from_utf8_lossy(&request);
            let total = data.len();
            if text.starts_with("HEAD") {
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(headers.as_bytes());
                continue;
            }
            let range = text.lines().find_map(|line| {
                let line = line.to_ascii_lowercase();
                line.strip_prefix("range: bytes=")
                    .map(|value| value.trim().to_string())
            });
            if matches!(mode, RangeMode::Honor) {
                if let Some(range) = range {
                    let (start, end) = range.split_once('-').unwrap();
                    let start: usize = start.parse().unwrap();
                    let end: usize = end.parse().unwrap();
                    let end = end.min(total - 1);
                    let body = &data[start..=end];
                    let headers = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{total}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(headers.as_bytes());
                    let _ = stream.write_all(body);
                    continue;
                }
            }
            let headers =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(headers.as_bytes());
            let _ = stream.write_all(&data);
        }
    });
    url
}
