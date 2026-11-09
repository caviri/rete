//! `rete card-url` reads the front of a file in one first read, and at most
//! one sized follow-up (SPEC §4.6) — counted at the server, not by the client.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// What the server saw: the `Range` header of every GET, and the bytes it sent.
#[derive(Default)]
struct Seen {
    ranges: Vec<String>,
    heads: usize,
    bytes: usize,
}

/// A range host that records every request it gets.
fn serve_counting(data: Vec<u8>) -> (String, Arc<Mutex<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/graph.rete", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Seen::default()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                request.push(byte[0]);
            }
            let text = String::from_utf8_lossy(&request).to_string();
            let total = data.len();
            if text.starts_with("HEAD") {
                log.lock().unwrap().heads += 1;
                let h = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(h.as_bytes());
                continue;
            }
            let range = text.lines().find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("range: bytes=")
                    .map(|v| v.trim().to_string())
            });
            let Some(range) = range else {
                let h = format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\n\r\n");
                let _ = stream.write_all(h.as_bytes());
                let _ = stream.write_all(&data);
                continue;
            };
            let (start, end) = range.split_once('-').unwrap();
            let start: usize = start.parse().unwrap();
            let end: usize = end.parse::<usize>().unwrap().min(total - 1);
            let body = &data[start..=end];
            {
                let mut s = log.lock().unwrap();
                s.ranges.push(range.clone());
                s.bytes += body.len();
            }
            let h = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{total}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(h.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    (url, seen)
}

/// A source of `n` statements with distinct literals, so the file is well past
/// the 64 KiB first read (and under the 8 MiB a plain open would fetch whole).
fn wordy_source(n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                "<http://example.test/s{i}> <http://example.test/p{}> \"value {i} {}\" .\n",
                i % 7,
                "lorem ipsum dolor sit amet ".repeat(2) + &i.to_string()
            )
        })
        .collect()
}

#[test]
fn a_small_file_costs_one_request() {
    let fixture = common::fixture();
    let carded = fixture.path("carded.rete");
    common::build(
        &fixture.source,
        &carded,
        &["--card", "--title", "Small remote"],
    );
    let image = std::fs::read(&carded).unwrap();
    assert!(image.len() < 64 * 1024, "the fixture is a small file");
    let (url, seen) = serve_counting(image);

    let out = common::rete()
        .arg("card-url")
        .arg(&url)
        .arg("--json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let card: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(card["title"], "Small remote");

    let seen = seen.lock().unwrap();
    assert_eq!(seen.ranges.len(), 1, "requests: {:?}", seen.ranges);
    assert_eq!(seen.heads, 0, "no HEAD is sent");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("in 1 range request(s)"),
        "the report says what went over the wire: {stderr}"
    );
}

#[test]
fn a_bigger_file_is_not_fetched_whole() {
    let fixture = common::fixture();
    let source = fixture.write("wordy.nt", wordy_source(20_000));
    let carded = fixture.path("wordy.rete");
    common::build(&source, &carded, &["--card", "--title", "Big remote"]);
    let image = std::fs::read(&carded).unwrap();
    assert!(image.len() > 128 * 1024, "{} bytes", image.len());
    // Under the reader's 8 MiB whole-file limit: a plain open would take the
    // rest of the file in a second read.
    assert!(image.len() < 8 << 20);
    let (url, seen) = serve_counting(image.clone());

    let out = common::rete()
        .arg("card-url")
        .arg(&url)
        .arg("--json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let card: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(card["title"], "Big remote");

    let seen = seen.lock().unwrap();
    assert!(
        (1..=2).contains(&seen.ranges.len()),
        "one first read and at most one follow-up, got {:?}",
        seen.ranges
    );
    assert_eq!(seen.ranges[0], "0-65535", "the first read is 64 KiB");
    assert!(
        seen.bytes < image.len() / 2,
        "{} of {} bytes were fetched",
        seen.bytes,
        image.len()
    );
    assert_eq!(seen.heads, 0, "no HEAD is sent");
}
