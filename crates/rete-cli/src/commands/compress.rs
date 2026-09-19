//! Streaming compression for `rete export`.
//!
//! # Why this is a stream and not a step
//!
//! A full N-Quads dump of a large graph runs to tens or hundreds of gibibytes of
//! text, and the export path is memory-bounded end to end (`--memory-budget-mb`,
//! #245-#248). Compressing it by buffering the document and then encoding would
//! throw that away in one line. So the codec sits *in* the writer chain —
//!
//! ```text
//!     statements -> BufWriter -> Encoder -> stdout
//! ```
//!
//! — and every byte is encoded as it is produced. Peak memory gains the codec's
//! own window (a few MiB at the levels anyone should use) and nothing else.
//!
//! # Finishing the frame
//!
//! The classic bug in this shape is dropping the encoder instead of finishing
//! it. Both zstd and gzip end a stream with a trailer; without it the output is a
//! *truncated frame* that looks like a file, copies like a file, and fails only
//! at decompression, possibly on someone else's machine days later. `Drop` cannot
//! report that failure — it has nowhere to return an error to — so:
//!
//! * [`close`] is the only way to end a stream, it calls the codec's `finish`,
//!   and it returns `io::Result`;
//! * the callers propagate that result rather than discarding it;
//! * and there is a test for the error path, not just the happy one, because a
//!   silently-truncated frame is exactly what a happy-path test cannot see.
//!
//! [`Encoder`] deliberately does **not** implement a `Drop` that finishes. A
//! `Drop` impl would make the failure disappear again, and it would make the
//! difference between "finished" and "abandoned" invisible at the call site.

use std::io::{self, BufWriter, Write};

/// Which codec `--compress` selects.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Codec {
    /// Write the bytes through unchanged — the default, and byte-for-byte the
    /// behaviour this path had before compression existed.
    #[default]
    None,
    /// zstd. The reason this feature exists: better ratio than gzip at several
    /// times the compression speed, and far faster to decompress.
    Zstd,
    /// gzip. Here because `flate2` was already a dependency (transparent `.gz`
    /// on build inputs), so it costs nothing to offer, and because some
    /// consumers still only take gzip.
    Gzip,
}

impl Codec {
    pub(crate) fn parse(name: &str) -> anyhow::Result<Self> {
        match name {
            "none" => Ok(Self::None),
            "zstd" => Ok(Self::Zstd),
            "gzip" => Ok(Self::Gzip),
            other => anyhow::bail!("unknown --compress codec: {other}"),
        }
    }

    /// The default level for this codec.
    ///
    /// For zstd this is **6**, not the library default of 3, and the choice is
    /// measured rather than inherited — see `dev/export-formats/compress.sh` and
    /// the table in the pull request. On the export output measured, 3 -> 6 buys
    /// a further ~7% off the file for ~35% more wall time, while 6 -> 12 buys
    /// ~2% more for over 4x the time. 6 is the knee.
    ///
    /// gzip keeps 6, which is both `flate2`'s and `gzip(1)`'s default and sits at
    /// the same kind of knee on the deflate curve.
    pub(crate) fn default_level(self) -> i32 {
        match self {
            Self::None => 0,
            Self::Zstd => 6,
            Self::Gzip => 6,
        }
    }

    /// The inclusive level range this codec accepts, for validating `--compress-level`.
    fn level_range(self) -> (i32, i32) {
        match self {
            Self::None => (0, 0),
            // zstd's negative levels trade ratio for speed and are legitimate on
            // a dump being written once and read once; 22 is the maximum without
            // `--ultra`, which needs a decoder window this does not negotiate.
            Self::Zstd => (-7, 22),
            Self::Gzip => (0, 9),
        }
    }

    /// The conventional file extension, for a diagnostic that tells the user what
    /// to name the file they are redirecting into.
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Zstd => ".zst",
            Self::Gzip => ".gz",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Zstd => "zstd",
            Self::Gzip => "gzip",
        }
    }
}

/// Validate a user-supplied level against the codec, so a typo fails before the
/// export starts rather than after an hour of scanning.
pub(crate) fn check_level(codec: Codec, level: Option<i32>) -> anyhow::Result<i32> {
    let Some(level) = level else {
        return Ok(codec.default_level());
    };
    if codec == Codec::None {
        anyhow::bail!(
            "--compress-level needs a codec; pass `--compress zstd` or `--compress gzip`"
        );
    }
    let (lo, hi) = codec.level_range();
    if level < lo || level > hi {
        anyhow::bail!(
            "--compress-level {level} is outside {}'s range {lo}..={hi}",
            codec.name()
        );
    }
    Ok(level)
}

/// One link in the output chain: the codec, or a pass-through.
pub(crate) enum Encoder<W: Write> {
    Plain(W),
    // Boxed: `zstd::Encoder` is large, and an enum is as big as its widest
    // variant, so without the box every `write` moves that much stack around.
    Zstd(Box<zstd::Encoder<'static, W>>),
    Gzip(flate2::write::GzEncoder<W>),
}

impl<W: Write> Write for Encoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(w) => w.write(buf),
            Self::Zstd(e) => e.write(buf),
            Self::Gzip(e) => e.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(w) => w.flush(),
            Self::Zstd(e) => e.flush(),
            Self::Gzip(e) => e.flush(),
        }
    }
}

/// The full chain a serializer writes into.
///
/// `BufWriter` on the OUTSIDE, so the codec is handed large blocks rather than
/// one `writeln!` at a time — which matters most for `Codec::None`, where it is
/// the only buffer, and where it keeps the uncompressed path byte-for-byte and
/// syscall-for-syscall what it was.
pub(crate) type Sink<W> = BufWriter<Encoder<W>>;

/// Begin a compressed (or pass-through) stream.
pub(crate) fn open<W: Write>(w: W, codec: Codec, level: i32) -> io::Result<Sink<W>> {
    let enc = match codec {
        Codec::None => Encoder::Plain(w),
        Codec::Zstd => Encoder::Zstd(Box::new(zstd::Encoder::new(w, level)?)),
        Codec::Gzip => Encoder::Gzip(flate2::write::GzEncoder::new(
            w,
            flate2::Compression::new(level.clamp(0, 9) as u32),
        )),
    };
    Ok(BufWriter::new(enc))
}

/// End the stream: flush the buffer, write the codec's trailer, and hand back
/// the sink.
///
/// **This is the only correct way to finish.** Dropping the `Sink` writes no
/// trailer and reports no error, producing a frame that fails at decompression
/// rather than here. Every caller propagates what this returns.
pub(crate) fn close<W: Write>(sink: Sink<W>) -> io::Result<W> {
    // `into_inner` flushes; its error type carries the buffered data away with
    // it, so unwrap it back into a plain io::Error rather than dropping the
    // cause.
    let enc = sink.into_inner().map_err(|e| e.into_error())?;
    match enc {
        Encoder::Plain(mut w) => {
            w.flush()?;
            Ok(w)
        }
        Encoder::Zstd(e) => e.finish(),
        Encoder::Gzip(e) => e.finish(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sink that fails after N bytes — for the error paths, which are the ones
    /// that matter here.
    struct Failing {
        budget: usize,
        written: usize,
    }

    impl Write for Failing {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.written >= self.budget {
                return Err(io::Error::other("disk full"));
            }
            let n = buf.len().min(self.budget - self.written);
            self.written += n;
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn roundtrip(codec: Codec, level: i32, payload: &[u8]) -> Vec<u8> {
        let mut sink = open(Vec::new(), codec, level).unwrap();
        sink.write_all(payload).unwrap();
        close(sink).unwrap()
    }

    #[test]
    fn none_is_a_pass_through() {
        assert_eq!(roundtrip(Codec::None, 0, b"hello"), b"hello");
    }

    #[test]
    fn zstd_output_decodes_back_to_the_input() {
        // Compressible, and long enough that the codec actually emits a block
        // rather than storing it raw.
        let payload: Vec<u8> =
            std::iter::repeat_n(b"<http://ex/s> <http://ex/p> <http://ex/o> .\n", 5000)
                .flatten()
                .copied()
                .collect();
        let out = roundtrip(Codec::Zstd, Codec::Zstd.default_level(), &payload);
        assert!(
            out.len() < payload.len() / 10,
            "should compress: {}",
            out.len()
        );
        // Magic number of a zstd frame (RFC 8878 §3.1.1).
        assert_eq!(&out[..4], &[0x28, 0xB5, 0x2F, 0xFD]);
        assert_eq!(zstd::decode_all(&out[..]).unwrap(), payload);
    }

    #[test]
    fn gzip_output_decodes_back_to_the_input() {
        use std::io::Read;
        let payload: Vec<u8> =
            std::iter::repeat_n(b"<http://ex/s> <http://ex/p> <http://ex/o> .\n", 5000)
                .flatten()
                .copied()
                .collect();
        let out = roundtrip(Codec::Gzip, Codec::Gzip.default_level(), &payload);
        assert_eq!(&out[..2], &[0x1F, 0x8B], "gzip magic");
        let mut got = Vec::new();
        flate2::read::GzDecoder::new(&out[..])
            .read_to_end(&mut got)
            .unwrap();
        assert_eq!(got, payload);
    }

    /// The whole reason `close` exists. A dropped encoder writes no trailer, so
    /// the bytes are a truncated frame — this pins the difference, so nobody
    /// "simplifies" `close` into a `Drop` impl later.
    #[test]
    fn a_dropped_encoder_produces_a_frame_that_does_not_decode() {
        let payload: Vec<u8> = std::iter::repeat_n(b"abcdefghij", 5000)
            .flatten()
            .copied()
            .collect();

        let finished = roundtrip(Codec::Zstd, 3, &payload);
        assert_eq!(zstd::decode_all(&finished[..]).unwrap(), payload);

        // Same writes, but the sink is dropped instead of closed. We have to
        // reach the inner Vec some other way, so write into a shared buffer.
        let abandoned = {
            struct Shared(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);
            impl Write for Shared {
                fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                    self.0.borrow_mut().extend_from_slice(b);
                    Ok(b.len())
                }
                fn flush(&mut self) -> io::Result<()> {
                    Ok(())
                }
            }
            let buf = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            {
                let mut sink = open(Shared(buf.clone()), Codec::Zstd, 3).unwrap();
                sink.write_all(&payload).unwrap();
                sink.flush().unwrap();
                drop(sink);
            }
            let v = buf.borrow().clone();
            v
        };
        assert!(
            zstd::decode_all(&abandoned[..]).is_err(),
            "an unfinished frame must NOT decode — if this ever passes, the \
             truncation bug close() guards against has become invisible"
        );
        assert_ne!(abandoned, finished);
    }

    #[test]
    fn a_write_failure_surfaces_from_close_rather_than_being_swallowed() {
        for codec in [Codec::None, Codec::Zstd, Codec::Gzip] {
            let mut sink = open(
                Failing {
                    budget: 8,
                    written: 0,
                },
                codec,
                codec.default_level(),
            )
            .unwrap();
            // Much more than the sink will accept. The BufWriter may absorb this
            // without error; the failure then has to come out of `close`.
            let big = vec![b'x'; 1 << 20];
            let wrote = sink.write_all(&big);
            let closed = close(sink);
            assert!(
                wrote.is_err() || closed.is_err(),
                "{codec:?}: a failing sink must produce an error somewhere"
            );
        }
    }

    #[test]
    fn level_validation_rejects_what_the_codec_would_not_accept() {
        assert_eq!(check_level(Codec::Zstd, None).unwrap(), 6);
        assert_eq!(check_level(Codec::Gzip, None).unwrap(), 6);
        assert_eq!(check_level(Codec::Zstd, Some(1)).unwrap(), 1);
        assert_eq!(check_level(Codec::Zstd, Some(-5)).unwrap(), -5);
        assert!(check_level(Codec::Zstd, Some(23)).is_err());
        assert!(check_level(Codec::Gzip, Some(10)).is_err());
        // A level with no codec is a mistake worth naming, not a silent no-op.
        assert!(check_level(Codec::None, Some(3)).is_err());
        assert_eq!(check_level(Codec::None, None).unwrap(), 0);
    }

    #[test]
    fn every_level_in_range_produces_a_decodable_frame() {
        let payload: Vec<u8> = std::iter::repeat_n(b"<http://ex/s> <http://ex/p> \"v\" .\n", 2000)
            .flatten()
            .copied()
            .collect();
        for level in [-7, -1, 1, 3, 6, 12, 19, 22] {
            let out = roundtrip(Codec::Zstd, level, &payload);
            assert_eq!(
                zstd::decode_all(&out[..]).unwrap(),
                payload,
                "zstd level {level}"
            );
        }
    }
}
