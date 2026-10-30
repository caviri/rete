//! Offline compression measurements over a built `.rete` (see
//! `docs/BENCHMARK.md`, "Compression (0.4)"). Reads the file's dictionary
//! chunks and index tiles, decompresses each one, and re-encodes the raw units
//! under other settings — nothing here writes a `.rete`.
//!
//! ```text
//! cargo run --release -p rete-core --example compression_lab -- FILE inventory
//! cargo run --release -p rete-core --example compression_lab -- FILE levels 3,9,15,19
//! cargo run --release -p rete-core --example compression_lab -- FILE dicts 16,64,112
//! cargo run --release -p rete-core --example compression_lab -- FILE hotsim 127,1024,16384
//! cargo run --release -p rete-core --example compression_lab -- FILE dictfixture idx:SOP 64 8 OUT
//! ```
//!
//! * `inventory` — units per section: count, raw and stored bytes, and the
//!   window the stored zstd frames declare.
//! * `levels` — every unit re-compressed at each level, as a streaming frame
//!   (what the writer emits) and as a sized single-segment frame; single-thread
//!   encode time; decode time with the C library (streaming and one-shot) and
//!   with `ruzstd` (a fresh `StreamingDecoder` per unit, as the wasm reader does,
//!   and one reused `FrameDecoder`).
//! * `dicts` — a zstd dictionary trained per section (each dictionary section,
//!   each permutation) at the given sizes in KiB; every unit compressed at level
//!   9 with it; bytes including the dictionary; decode time with C and `ruzstd`.
//! * `hotsim` — what frequency-ordered object ids would save: the `H` most
//!   frequent object-only terms renumbered `1..=H` (a "hot" section in front of
//!   the shared one, every other id shifted by `H`), all permutations re-tiled at
//!   64 KiB and re-compressed at level 9, against the same re-tiling of the
//!   original ids.
//! * `dictfixture` — a trained dictionary plus a few units compressed with it
//!   and their raw bytes, written to a directory for a decoder check elsewhere
//!   (the `ruzstd`-in-wasm check of the experiment note).
//!
//! Output is tab-separated, one record per line, for the experiment scripts.

use std::io::Read;
use std::time::Instant;

use rete_core::header::Header;
use rete_core::triples::{TripleBlock, TripleBlockBuilder};
use rete_core::varint::read_uvarint;

/// One independently compressed unit: a dictionary chunk or an index tile.
struct Unit {
    raw: Vec<u8>,
    stored: usize,
    /// window declared by the stored frame (`None` = single-segment)
    window: Option<u64>,
}

/// A named group of units (one dictionary section or one permutation).
struct Group {
    name: String,
    units: Vec<Unit>,
}

fn uv(b: &[u8], pos: &mut usize) -> u64 {
    let (v, n) = read_uvarint(&b[*pos..]).expect("truncated varint");
    *pos += n;
    v
}

/// `[count][len, payload]*` — the outer container of the dictionary and index.
fn container(b: &[u8]) -> Vec<&[u8]> {
    let mut pos = 0;
    let n = uv(b, &mut pos);
    (0..n)
        .map(|_| {
            let len = uv(b, &mut pos) as usize;
            let s = &b[pos..pos + len];
            pos += len;
            s
        })
        .collect()
}

fn unzstd(b: &[u8]) -> Vec<u8> {
    zstd::decode_all(b).expect("stored frame decodes")
}

/// A chunked dictionary section: `(term_count, chunks)`.
fn dict_section(p: &[u8]) -> (u64, Vec<Unit>) {
    let mut pos = 0;
    let hlen = uv(p, &mut pos) as usize;
    let header = &p[pos..pos + hlen];
    let term_count = read_uvarint(header).map(|(v, _)| v).unwrap_or(0);
    pos += hlen;
    let n = uv(p, &mut pos);
    let mut lens = Vec::new();
    for _ in 0..n {
        let _first_run = uv(p, &mut pos);
        let klen = uv(p, &mut pos) as usize;
        pos += klen;
        lens.push(uv(p, &mut pos) as usize);
    }
    let units = lens
        .into_iter()
        .map(|len| {
            let c = &p[pos..pos + len];
            pos += len;
            Unit {
                raw: unzstd(c),
                stored: len,
                window: frame_window(c),
            }
        })
        .collect();
    (term_count, units)
}

/// A tiled permutation section (the synopsis trailer is not a unit).
fn tiled_section(p: &[u8]) -> Vec<Unit> {
    let mut pos = 0;
    let n = uv(p, &mut pos);
    let mut lens = Vec::new();
    for _ in 0..n {
        let _dmin = uv(p, &mut pos);
        let _span = uv(p, &mut pos);
        lens.push(uv(p, &mut pos) as usize);
    }
    lens.into_iter()
        .map(|len| {
            let c = &p[pos..pos + len];
            pos += len;
            Unit {
                raw: unzstd(c),
                stored: len,
                window: frame_window(c),
            }
        })
        .collect()
}

const DICT_NAMES: [&str; 4] = [
    "dict:shared",
    "dict:subjects",
    "dict:objects",
    "dict:predicates",
];

/// `(a, b, c)` slots of each permutation as canonical roles (0=s, 1=p, 2=o).
fn roles(name: &str) -> [usize; 3] {
    match name {
        "SPO" => [0, 1, 2],
        "SOP" => [0, 2, 1],
        "PSO" => [1, 0, 2],
        "POS" => [1, 2, 0],
        "OSP" => [2, 0, 1],
        "OPS" => [2, 1, 0],
        _ => panic!("unknown permutation {name}"),
    }
}

struct Loaded {
    groups: Vec<Group>,
    /// term counts of the four dictionary sections
    counts: [u64; 4],
}

fn load(path: &str) -> Loaded {
    let bytes = std::fs::read(path).expect("read file");
    let h = Header::from_bytes(&bytes).expect("header");
    let d = &bytes[h.dictionary_offset as usize..(h.dictionary_offset + h.dictionary_len) as usize];
    let mut groups = Vec::new();
    let mut counts = [0u64; 4];
    for (i, sec) in container(d).into_iter().enumerate() {
        let (tc, units) = dict_section(sec);
        counts[i] = tc;
        groups.push(Group {
            name: DICT_NAMES[i].to_string(),
            units,
        });
    }
    let x = &bytes[h.root_dir_offset as usize..(h.root_dir_offset + h.root_dir_len) as usize];
    for (name, sec) in h.perms.names().into_iter().zip(container(x)) {
        groups.push(Group {
            name: format!("idx:{name}"),
            units: tiled_section(sec),
        });
    }
    Loaded { groups, counts }
}

/// The window a zstd frame header declares (`None` for a single-segment frame,
/// whose window is its content size).
fn frame_window(f: &[u8]) -> Option<u64> {
    if f.len() < 6 || f[..4] != [0x28, 0xB5, 0x2F, 0xFD] {
        return None;
    }
    let fhd = f[4];
    if fhd & 0x20 != 0 {
        return None;
    }
    let wd = f[5];
    let log = 10 + (wd >> 3) as u32;
    let base = 1u64 << log;
    Some(base + (base / 8) * (wd & 7) as u64)
}

fn kind(name: &str) -> &'static str {
    if name.starts_with("dict:") {
        "dict"
    } else {
        "index"
    }
}

fn inventory(l: &Loaded) {
    println!("rec\tgroup\tunits\traw_bytes\tstored_bytes\tratio\tmax_raw\twindow");
    for g in &l.groups {
        let raw: usize = g.units.iter().map(|u| u.raw.len()).sum();
        let stored: usize = g.units.iter().map(|u| u.stored).sum();
        let max_raw = g.units.iter().map(|u| u.raw.len()).max().unwrap_or(0);
        let window = match g.units.first().map(|u| u.window) {
            Some(Some(w)) => w.to_string(),
            Some(None) => "single-segment".to_string(),
            None => "-".to_string(),
        };
        println!(
            "inv\t{}\t{}\t{}\t{}\t{:.3}\t{}\t{window}",
            g.name,
            g.units.len(),
            raw,
            stored,
            raw as f64 / stored.max(1) as f64,
            max_raw
        );
    }
    println!("counts\t{:?}", l.counts);
}

/// Best-of-3 seconds to run `f` over every frame, plus the total output length
/// (so the work cannot be optimized away). The machine is shared, so the best
/// of three passes is the least disturbed one.
fn time_decode(frames: &[Vec<u8>], mut f: impl FnMut(usize, &[u8]) -> usize) -> (f64, usize) {
    let mut best = f64::MAX;
    let mut n = 0;
    for _ in 0..3 {
        let t = Instant::now();
        n = 0;
        for (i, fr) in frames.iter().enumerate() {
            n += f(i, fr);
        }
        best = best.min(t.elapsed().as_secs_f64());
    }
    (best, n)
}

fn decoders(label: &str, frames: &[Vec<u8>], raws: &[&[u8]]) {
    let raw_total: usize = raws.iter().map(|r| r.len()).sum();
    let mb = raw_total as f64 / 1e6;
    let caps: Vec<usize> = raws.iter().map(|r| r.len()).collect();
    let (t_cs, n1) = time_decode(frames, |_, f| zstd::decode_all(f).unwrap().len());
    let (t_cb, n2) = time_decode(frames, |i, f| {
        zstd::bulk::decompress(f, caps[i]).unwrap().len()
    });
    let (t_rs, n3) = time_decode(frames, |_, f| {
        let mut d = ruzstd::decoding::StreamingDecoder::new(f).unwrap();
        let mut out = Vec::new();
        d.read_to_end(&mut out).unwrap();
        out.len()
    });
    let mut fd = ruzstd::decoding::FrameDecoder::new();
    let (t_rf, n4) = time_decode(frames, |i, f| {
        let mut out = Vec::with_capacity(caps[i]);
        fd.decode_all_to_vec(f, &mut out).unwrap();
        out.len()
    });
    assert!(n1 == raw_total && n2 == raw_total && n3 == raw_total && n4 == raw_total);
    println!(
        "dec\t{label}\t{}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.2}\t{:.2}\t{:.2}\t{:.2}",
        frames.len(),
        mb / t_cs,
        mb / t_cb,
        mb / t_rs,
        mb / t_rf,
        t_cs * 1e6 / frames.len() as f64,
        t_cb * 1e6 / frames.len() as f64,
        t_rs * 1e6 / frames.len() as f64,
        t_rf * 1e6 / frames.len() as f64,
    );
}

fn levels(l: &Loaded, list: &[i32]) {
    println!("rec\tlevel\tframe\tkind\traw_bytes\tcomp_bytes\tenc_s\tenc_MBps");
    println!("rec\tlabel\tunits\tC_stream_MBps\tC_oneshot_MBps\truzstd_stream_MBps\truzstd_reuse_MBps\tC_stream_us\tC_oneshot_us\truzstd_stream_us\truzstd_reuse_us");
    for &lv in list {
        for sized in [false, true] {
            for k in ["dict", "index"] {
                let raws: Vec<&[u8]> = l
                    .groups
                    .iter()
                    .filter(|g| kind(&g.name) == k)
                    .flat_map(|g| g.units.iter().map(|u| u.raw.as_slice()))
                    .collect();
                let t = Instant::now();
                let frames: Vec<Vec<u8>> = raws
                    .iter()
                    .map(|r| {
                        if sized {
                            zstd::bulk::compress(r, lv).unwrap()
                        } else {
                            zstd::encode_all(*r, lv).unwrap()
                        }
                    })
                    .collect();
                let enc = t.elapsed().as_secs_f64();
                let raw_total: usize = raws.iter().map(|r| r.len()).sum();
                let comp: usize = frames.iter().map(|f| f.len()).sum();
                let frame = if sized { "sized" } else { "stream" };
                println!(
                    "lvl\t{lv}\t{frame}\t{k}\t{raw_total}\t{comp}\t{enc:.2}\t{:.1}",
                    raw_total as f64 / 1e6 / enc
                );
                decoders(&format!("L{lv}-{frame}-{k}"), &frames, &raws);
            }
        }
    }
}

fn dicts(l: &Loaded, sizes_kib: &[usize]) {
    println!("rec\tgroup\tdict_kib\tunits\traw\tstored_L9\tsized_L9_nodict\twith_dict\tdict_bytes\ttotal\ttrain_s\tC_dict_MBps\truzstd_dict_MBps\truzstd_nodict_MBps");
    for g in &l.groups {
        let raws: Vec<&[u8]> = g.units.iter().map(|u| u.raw.as_slice()).collect();
        let raw_total: usize = raws.iter().map(|r| r.len()).sum();
        let stored: usize = g.units.iter().map(|u| u.stored).sum();
        let nodict: Vec<Vec<u8>> = raws
            .iter()
            .map(|r| zstd::bulk::compress(r, 9).unwrap())
            .collect();
        let nodict_total: usize = nodict.iter().map(|f| f.len()).sum();
        let mb = raw_total as f64 / 1e6;
        // ruzstd without a dictionary, reused decoder (the fair baseline)
        let mut fd0 = ruzstd::decoding::FrameDecoder::new();
        let (t_rz0, _) = time_decode(&nodict, |i, f| {
            let mut out = Vec::with_capacity(raws[i].len());
            fd0.decode_all_to_vec(f, &mut out).unwrap();
            out.len()
        });
        let rz0 = mb / t_rz0;
        for &kib in sizes_kib {
            let max = kib * 1024;
            if raws.len() < 8 || raw_total < max * 4 {
                println!("dic\t{}\t{kib}\t{}\t{raw_total}\t{stored}\t{nodict_total}\t-\t-\t-\t-\t-\t-\t{rz0:.1}", g.name, raws.len());
                continue;
            }
            // Train on an evenly spaced sample of at most ~200x the dictionary.
            let budget = (max * 200).max(1 << 20);
            let step = (raw_total / budget).max(1);
            let sample: Vec<&[u8]> = raws.iter().step_by(step).copied().collect();
            let t = Instant::now();
            let dict = match zstd::dict::from_samples(&sample, max) {
                Ok(d) => d,
                Err(e) => {
                    println!("dic\t{}\t{kib}\ttrain-failed:{e}", g.name);
                    continue;
                }
            };
            let train = t.elapsed().as_secs_f64();
            let mut c = zstd::bulk::Compressor::with_dictionary(9, &dict).unwrap();
            let frames: Vec<Vec<u8>> = raws.iter().map(|r| c.compress(r).unwrap()).collect();
            let with: usize = frames.iter().map(|f| f.len()).sum();
            // decode: C with the dictionary
            let mut dc = zstd::bulk::Decompressor::with_dictionary(&dict).unwrap();
            let (t_cd, _) = time_decode(&frames, |i, f| {
                let v = dc.decompress(f, raws[i].len()).unwrap();
                assert_eq!(v.len(), raws[i].len());
                v.len()
            });
            let cd = mb / t_cd;
            // decode: ruzstd with the dictionary (the wasm decoder), checked byte for byte
            let mut fd = ruzstd::decoding::FrameDecoder::new();
            fd.add_dict(ruzstd::decoding::Dictionary::decode_dict(&dict).unwrap())
                .unwrap();
            let (t_rz, _) = time_decode(&frames, |i, f| {
                let mut out = Vec::with_capacity(raws[i].len());
                fd.decode_all_to_vec(f, &mut out).unwrap();
                assert!(
                    out.as_slice() == raws[i],
                    "ruzstd dictionary decode mismatch"
                );
                out.len()
            });
            let rz = mb / t_rz;
            println!(
                "dic\t{}\t{kib}\t{}\t{raw_total}\t{stored}\t{nodict_total}\t{with}\t{}\t{}\t{train:.1}\t{cd:.1}\t{rz:.1}\t{rz0:.1}",
                g.name,
                raws.len(),
                dict.len(),
                with + dict.len()
            );
        }
    }
}

/// Encoded size of one a-group of a tiled section (the triple-block layout of
/// SPEC §6.1): `Δa, num_b, (Δb, num_c, Δc*)*`.
fn vlen(v: u32) -> usize {
    match v {
        0..=0x7f => 1,
        0x80..=0x3fff => 2,
        0x4000..=0x1f_ffff => 3,
        0x20_0000..=0xfff_ffff => 4,
        _ => 5,
    }
}

/// Greedy re-tiling of sorted `(a, b, c)` triples to `budget` encoded bytes
/// (whole a-groups; a group over budget is cut where it crosses the budget),
/// each tile encoded as a triple block and compressed at level 9. Returns the
/// compressed bytes and the tile count.
fn retile(sorted: &[(u32, u32, u32)], budget: usize) -> (usize, usize) {
    let mut total = 0;
    let mut tiles = 0;
    let mut start = 0;
    let mut size = 0usize;
    let mut i = 0;
    let mut prev_a = 0u32;
    let flush = |s: usize, e: usize, total: &mut usize, tiles: &mut usize| {
        if e > s {
            let mut b = TripleBlockBuilder::new();
            for &t in &sorted[s..e] {
                b.push(t);
            }
            let raw = b.build();
            *total += zstd::encode_all(raw.as_slice(), 9).unwrap().len();
            *tiles += 1;
        }
    };
    while i < sorted.len() {
        let a = sorted[i].0;
        let g0 = i;
        let mut gsize = vlen(a - prev_a) + 1;
        let mut prev_b = 0u32;
        while i < sorted.len() && sorted[i].0 == a {
            let b = sorted[i].1;
            gsize += vlen(b - prev_b) + 1;
            prev_b = b;
            let mut prev_c = 0u32;
            while i < sorted.len() && sorted[i].0 == a && sorted[i].1 == b {
                gsize += vlen(sorted[i].2 - prev_c);
                prev_c = sorted[i].2;
                i += 1;
                if gsize > budget {
                    flush(start, i, &mut total, &mut tiles);
                    start = i;
                    size = 0;
                    gsize = 0;
                }
            }
        }
        if size + gsize > budget && start < g0 {
            flush(start, g0, &mut total, &mut tiles);
            start = g0;
            size = 0;
        }
        size += gsize;
        prev_a = a;
    }
    flush(start, sorted.len(), &mut total, &mut tiles);
    (total, tiles)
}

fn hotsim(l: &Loaded, hs: &[usize]) {
    let s_cnt = l.counts[0] as u32;
    let su = l.counts[1] as u32;
    let oo = l.counts[2] as u32;
    // canonical triples from SPO
    let spo = l
        .groups
        .iter()
        .find(|g| g.name == "idx:SPO")
        .expect("SPO section");
    let mut triples: Vec<(u32, u32, u32)> = Vec::new();
    for u in &spo.units {
        triples.extend(TripleBlock::parse(&u.raw).expect("tile").triples());
    }
    eprintln!(
        "hotsim: {} triples, S={s_cnt} Su={su} Oo={oo}",
        triples.len()
    );
    // object-only frequencies
    let mut freq = vec![0u64; oo as usize + 1];
    for &(_, _, o) in &triples {
        if o > s_cnt {
            freq[(o - s_cnt) as usize] += 1;
        }
    }
    let mut order: Vec<u32> = (1..=oo).collect();
    order.sort_by(|a, b| freq[*b as usize].cmp(&freq[*a as usize]).then(a.cmp(b)));
    let perms = ["SPO", "POS", "OSP", "SOP", "PSO", "OPS"];
    println!("rec\tH\thot_share_of_objects\tperm\ttiles\tcomp_bytes");
    for &h in std::iter::once(&0usize).chain(hs.iter()) {
        let h = h.min(oo as usize) as u32;
        // object-only local id -> new local rank among the non-hot (0 = hot)
        let mut hot_rank = vec![0u32; oo as usize + 1];
        for (r, &id) in order.iter().take(h as usize).enumerate() {
            hot_rank[id as usize] = r as u32 + 1;
        }
        let mut rest = vec![0u32; oo as usize + 1];
        let mut k = 0u32;
        for id in 1..=oo {
            if hot_rank[id as usize] == 0 {
                k += 1;
                rest[id as usize] = k;
            }
        }
        let hot_objs: u64 = order
            .iter()
            .take(h as usize)
            .map(|&i| freq[i as usize])
            .sum();
        let map_s = |s: u32| s + h;
        let map_o = |o: u32| {
            if o <= s_cnt {
                o + h
            } else {
                let loc = o - s_cnt;
                match hot_rank[loc as usize] {
                    0 => s_cnt + h + rest[loc as usize],
                    r => r,
                }
            }
        };
        let canon: Vec<(u32, u32, u32)> = if h == 0 {
            triples.clone()
        } else {
            triples
                .iter()
                .map(|&(s, p, o)| (map_s(s), p, map_o(o)))
                .collect()
        };
        let mut total = 0;
        for name in perms {
            let r = roles(name);
            let mut v: Vec<(u32, u32, u32)> = canon
                .iter()
                .map(|&(s, p, o)| {
                    let c = [s, p, o];
                    (c[r[0]], c[r[1]], c[r[2]])
                })
                .collect();
            v.sort_unstable();
            let (bytes, tiles) = retile(&v, 64 * 1024);
            total += bytes;
            println!(
                "hot\t{h}\t{:.4}\t{name}\t{tiles}\t{bytes}",
                hot_objs as f64 / triples.len() as f64
            );
        }
        println!(
            "hot\t{h}\t{:.4}\tALL\t-\t{total}",
            hot_objs as f64 / triples.len() as f64
        );
    }
}

/// Write a trained dictionary and `n` evenly spaced units of one group, each
/// in four frame configurations, for a decoder check outside this process (the
/// `ruzstd`-in-wasm check): `OUT/dict.bin`, `OUT/raw-I.bin` and
/// `OUT/{stream9,sized9,sized19,dict9}-I.zst`.
fn dictfixture(l: &Loaded, group: &str, kib: usize, n: usize, out: &str) {
    let g = l
        .groups
        .iter()
        .find(|g| g.name == group)
        .expect("no such group");
    let raws: Vec<&[u8]> = g.units.iter().map(|u| u.raw.as_slice()).collect();
    let dict = zstd::dict::from_samples(&raws, kib * 1024).expect("train");
    let mut c = zstd::bulk::Compressor::with_dictionary(9, &dict).unwrap();
    std::fs::create_dir_all(out).unwrap();
    std::fs::write(format!("{out}/dict.bin"), &dict).unwrap();
    let step = (raws.len() / n.max(1)).max(1);
    let mut sizes = [0usize; 4];
    for (i, r) in raws.iter().step_by(step).take(n).enumerate() {
        std::fs::write(format!("{out}/raw-{i}.bin"), r).unwrap();
        let frames = [
            ("stream9", zstd::encode_all(*r, 9).unwrap()),
            ("sized9", zstd::bulk::compress(r, 9).unwrap()),
            ("sized19", zstd::bulk::compress(r, 19).unwrap()),
            ("dict9", c.compress(r).unwrap()),
        ];
        for (k, (name, f)) in frames.iter().enumerate() {
            sizes[k] += f.len();
            std::fs::write(format!("{out}/{name}-{i}.zst"), f).unwrap();
        }
    }
    println!("fixture	{group}	{kib}	{}	{out}	{sizes:?}", dict.len());
}

fn list<T: std::str::FromStr>(s: Option<&String>, default: &str) -> Vec<T> {
    s.map(String::as_str)
        .unwrap_or(default)
        .split(',')
        .filter_map(|x| x.trim().parse().ok())
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: compression_lab FILE inventory|levels L,..|dicts KIB,..|hotsim H,..");
        std::process::exit(2);
    }
    let t = Instant::now();
    let l = load(&args[1]);
    eprintln!("loaded {} in {:.1}s", args[1], t.elapsed().as_secs_f64());
    match args[2].as_str() {
        "inventory" => inventory(&l),
        "levels" => levels(&l, &list(args.get(3), "3,9,15,19")),
        "dicts" => dicts(&l, &list(args.get(3), "16,64,112")),
        "hotsim" => hotsim(&l, &list(args.get(3), "127,1024,16384")),
        "dictfixture" => dictfixture(
            &l,
            &args[3],
            args[4].parse().unwrap(),
            args[5].parse().unwrap(),
            &args[6],
        ),
        other => {
            eprintln!("unknown command {other}");
            std::process::exit(2);
        }
    }
}
