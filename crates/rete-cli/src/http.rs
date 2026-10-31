//! The HTTP(S) range reader behind every URL command. It lives in rete-core
//! (`rete_core::http`, the `http` feature) so the Python and R clients read
//! remote files through the same code: kept-alive connections, parallel and
//! coalesced batches, retries, and the bounded whole-body fallback for a host
//! that ignores `Range`. Its own tests are there; this module keeps the one
//! property that belongs to the CLI — that the URL commands' reader hands the
//! engine's batches through whole.

pub use rete_core::http::HttpRangeReader;

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use rete_core::RangeReader;

    /// A localhost range host that holds every GET for 30 ms and records how
    /// many it had in flight at once.
    fn serve(data: Vec<u8>) -> (String, Arc<AtomicU64>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let (data, max) = (Arc::new(data), Arc::new(AtomicU64::new(0)));
        let (in_flight, max_seen) = (Arc::new(AtomicU64::new(0)), max.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let (data, in_flight, max_seen) =
                    (data.clone(), in_flight.clone(), max_seen.clone());
                std::thread::spawn(move || {
                    let mut req = Vec::new();
                    let mut byte = [0u8; 1];
                    while !req.ends_with(b"\r\n\r\n") {
                        if stream.read(&mut byte).unwrap_or(0) == 0 {
                            return;
                        }
                        req.push(byte[0]);
                    }
                    let text = String::from_utf8_lossy(&req).to_ascii_lowercase();
                    let total = data.len();
                    if text.starts_with("head") {
                        let h = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(h.as_bytes());
                        return;
                    }
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_seen.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(30));
                    let r = text
                        .lines()
                        .find_map(|l| l.strip_prefix("range: bytes="))
                        .unwrap()
                        .trim()
                        .to_string();
                    let (a, b) = r.split_once('-').unwrap();
                    let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
                    let slice = &data[a..=b.min(total - 1)];
                    let h = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {a}-{b}/{total}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        slice.len()
                    );
                    let _ = stream.write_all(h.as_bytes());
                    let _ = stream.write_all(slice);
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        (url, max)
    }

    /// The URL commands reach the HTTP reader through `RangedSourceReader`;
    /// it must hand them the batch (and the fan-out), not split it.
    #[test]
    fn ranged_source_hands_batches_to_the_http_reader() {
        use crate::commands::range_source::RangedSourceReader;
        let data: Vec<u8> = (0..=255u8).cycle().take(1 << 20).collect();
        let (url, max_in_flight) = serve(data.clone());
        let r = RangedSourceReader::open(&url).unwrap();
        assert!(r.concurrency() > 1);
        let ranges: Vec<(u64, u64)> = (0..8).map(|i| (i * 131_072, 64)).collect();
        let got = r.read_many(&ranges).unwrap();
        for (&(o, l), b) in ranges.iter().zip(&got) {
            assert_eq!(b.as_slice(), &data[o as usize..(o + l) as usize]);
        }
        assert!(
            max_in_flight.load(Ordering::SeqCst) > 1,
            "the batch went out one range at a time"
        );
    }
}
