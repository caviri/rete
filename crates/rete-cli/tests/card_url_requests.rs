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
/// the 128 KiB first read (and under the 8 MiB a plain open would fetch whole).
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
    assert!(image.len() < 128 * 1024, "the fixture is a small file");
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
    assert_eq!(seen.ranges[0], "0-131071", "the first read is 128 KiB");
    assert!(
        seen.bytes < image.len() / 2,
        "{} of {} bytes were fetched",
        seen.bytes,
        image.len()
    );
    assert_eq!(seen.heads, 0, "no HEAD is sent");
}

/// A card with `queries` long example queries: the card (and so the front)
/// grows with it, which is how a test gets a front of a chosen size.
fn wide_card(queries: usize) -> String {
    // Pseudo-random letters: a card of repeated words would compress away.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut letter = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        char::from(b'a' + (state % 26) as u8)
    };
    let qs: Vec<String> = (0..queries)
        .map(|i| {
            let words: Vec<String> = (0..40)
                .map(|_| (0..6).map(|_| letter()).collect())
                .collect();
            format!(
                "SELECT ?s WHERE {{ ?s <http://example.test/q{i}> \"{}\" }}",
                words.join(" ")
            )
        })
        .collect();
    serde_json::json!({ "title": "Wide card", "example_queries": qs }).to_string()
}

/// `(front_base, card_end, record_end)` of a `0x06` image, read off its header.
fn front_of(image: &[u8]) -> (u64, u64, u64) {
    let u = |at: usize| u64::from(u32::from_le_bytes(image[at..at + 4].try_into().unwrap()));
    let region = 128 + u(32);
    (
        region + u(36),
        region + u(80) + u(84),
        region + u(104) + u(108),
    )
}

/// Build a file whose card is `queries` example queries wide, over a source
/// big enough that the file is well past the first read.
fn wide_file(fixture: &common::Fixture, queries: usize) -> Vec<u8> {
    let source = fixture.write("wide.nt", wordy_source(4_000));
    let card = fixture.write("wide.json", wide_card(queries));
    let out = fixture.path("wide.rete");
    common::build(&source, &out, &["--card-file", card.to_str().unwrap()]);
    std::fs::read(&out).unwrap()
}

/// What the first read of 128 KiB buys: a front of 64 to 128 KiB (a big card,
/// the shape of the OpenAIRE and OpenCitations Meta fronts) is one request.
/// With the 64 KiB first read it was two.
#[test]
fn a_front_between_64_and_128_kib_is_one_request() {
    let fixture = common::fixture();
    let image = wide_file(&fixture, 150);
    let (front, _, record_end) = front_of(&image);
    assert!(
        (64 * 1024..128 * 1024).contains(&front),
        "fixture front {front}"
    );
    assert!(record_end <= front);
    assert!(image.len() > 128 * 1024, "{} bytes", image.len());
    let (url, seen) = serve_counting(image);

    let out = common::rete()
        .arg("card-url")
        .arg(&url)
        .arg("--json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let card: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(card["title"], "Wide card");

    let seen = seen.lock().unwrap();
    assert_eq!(seen.ranges, vec!["0-131071".to_string()]);
    assert_eq!(seen.bytes, 128 * 1024);
    assert_eq!(seen.heads, 0, "no HEAD is sent");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("in 1 range request(s)"), "{stderr}");
}

/// 0.5.6 read on to the end of the front (the whole card region, which holds
/// as much slack again as card and record). `card-url` now stops at the end of
/// the card, and of the record when it is printed: here the front is past the
/// first read and the card is not.
#[test]
fn card_url_stops_at_the_end_of_the_card() {
    let fixture = common::fixture();
    let image = wide_file(&fixture, 340);
    let (front, card_end, record_end) = front_of(&image);
    let first = 128 * 1024;
    assert!(front > first, "the front {front} is past the first read");
    assert!(record_end <= first, "the record ends at {record_end}");
    assert!(card_end < record_end);
    let (url, seen) = serve_counting(image);

    let out = common::rete()
        .arg("card-url")
        .arg(&url)
        .arg("--json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let card: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(card["title"], "Wide card");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.ranges, vec!["0-131071".to_string()]);
    assert_eq!(seen.bytes, first as usize, "{} bytes", seen.bytes);
}

/// When the record is printed and ends past the first read, the follow-up
/// stops there, not at the end of the front.
#[test]
fn the_follow_up_ends_at_the_record_not_the_front() {
    let fixture = common::fixture();
    let image = wide_file(&fixture, 705);
    let (front, card_end, record_end) = front_of(&image);
    let first = 128 * 1024;
    assert!(
        card_end < first && record_end > first,
        "{card_end} {record_end}"
    );
    assert!(front > record_end + 1024, "slack after the record");
    for (format, end) in [("json", record_end), ("croissant", card_end)] {
        let (url, seen) = serve_counting(image.clone());
        let out = common::rete()
            .arg("card-url")
            .arg(&url)
            .args(["--format", format])
            .output()
            .unwrap();
        assert!(out.status.success(), "{format}");
        let seen = seen.lock().unwrap();
        // Croissant never prints the record: the first read is enough.
        let want = if end <= first {
            vec!["0-131071".to_string()]
        } else {
            vec!["0-131071".to_string(), format!("131072-{}", end - 1)]
        };
        assert_eq!(seen.ranges, want, "{format}");
    }
}
