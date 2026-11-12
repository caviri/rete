//! `rete verify`'s check of the dictionary's term order (SPEC.md section 5.2).
//!
//! The digests say the bytes are the writer's; this says the sections are sorted
//! in the order the file's generation names, that a `0x08` root's group counts
//! are what the terms say, and that every chunk's routing key routes (it lies
//! above the previous chunk's last term and at or below its own first), which
//! is what `term -> id` relies on. The chunks of a derived IRI family that the
//! file does not store are rebuilt from the index and checked the same way.

use std::cmp::Ordering;
use std::collections::VecDeque;

use crate::file::FileError;
use crate::reader::RangeReader;
use crate::term_order::{GroupCounts, TermOrder};

const NAMES: [&str; 4] = ["shared", "subject-only", "object-only", "predicates"];

/// Walk the front-coded entries of one chunk body, handing each full term to
/// `f`. `None` on malformed bytes.
fn walk(body: &[u8], f: &mut dyn FnMut(&[u8])) -> Option<()> {
    let mut pos = 0usize;
    let mut buf: Vec<u8> = Vec::new();
    while pos < body.len() {
        let (shared, n1) = crate::varint::read_uvarint(body.get(pos..)?)?;
        let p = pos + n1;
        let (suf, n2) = crate::varint::read_uvarint(body.get(p..)?)?;
        let start = p + n2;
        let end = start.checked_add(suf as usize)?;
        let s = body.get(start..end)?;
        if shared as usize > buf.len() {
            return None;
        }
        buf.truncate(shared as usize);
        buf.extend_from_slice(s);
        f(&buf);
        pos = end;
    }
    Some(())
}

/// Check the four dictionary sections of a `0x06`, `0x07` or `0x08` file:
/// terms strictly ascending in the generation's order, the root's term count
/// and (`0x08`) group counts, and every routing key. Returns how many terms it
/// read and what is wrong (empty when the dictionary is sound). Reads every
/// dictionary chunk once; memory is one chunk (a batch of them, for the chunks
/// of a derived family).
pub fn check_dictionary(input: &dyn RangeReader) -> Result<(u64, Vec<String>), FileError> {
    let parts = super::parts::from_v06(input)?;
    let order = parts.order;
    let mut bad = Vec::new();
    let mut total = 0u64;
    let mut rebuilt: Option<(super::derived::Walk<'_>, VecDeque<super::derived::Rebuilt>)> =
        if parts.derived.is_empty() {
            None
        } else {
            let w = super::derived::Walk::new(&parts, input)?;
            bad.extend(w.problems.iter().cloned());
            Some((w, VecDeque::new()))
        };
    for (si, d) in parts.dict.iter().enumerate() {
        let name = NAMES[si];
        let mut prev: Option<Vec<u8>> = None;
        let mut counts = GroupCounts::default();
        let mut seen = 0u64;
        let mut unsorted = 0u64;
        for (ci, (c, off)) in d.chunks.iter().enumerate() {
            // The terms of the chunk: its body decoded, or rebuilt.
            let mut from_index: Option<Vec<Vec<u8>>> = None;
            let mut body = Vec::new();
            match rebuilt.as_mut() {
                Some((w, queue)) if c.clen == 0 && si == crate::derived::SECTION => {
                    if queue.front().is_none_or(|q| q.0 < ci) {
                        queue.clear();
                        queue.extend(w.batch()?);
                    }
                    match queue.pop_front() {
                        Some((n, Some(t))) if n == ci => from_index = Some(t),
                        _ => {
                            bad.push(format!(
                                "{name} chunk {ci}: its terms do not rebuild from the index"
                            ));
                            continue;
                        }
                    }
                }
                _ => {
                    let stored = input.read_at(*off, c.clen)?;
                    body = match crate::file::decompress(parts.dict_codec, &stored) {
                        Ok(b) => b,
                        Err(_) => {
                            bad.push(format!("{name} chunk {ci}: does not decompress"));
                            continue;
                        }
                    };
                }
            }
            let mut first: Option<Vec<u8>> = None;
            let mut last = prev.take().unwrap_or_default();
            let mut have_last = !last.is_empty() || ci > 0;
            let before = last.clone();
            let mut visit = |t: &[u8]| {
                if first.is_none() {
                    first = Some(t.to_vec());
                }
                if have_last && order.cmp(&last, t) != Ordering::Less {
                    unsorted += 1;
                }
                last.clear();
                last.extend_from_slice(t);
                have_last = true;
                counts.add(t);
                seen += 1;
            };
            let ok = match &from_index {
                Some(terms) => {
                    terms.iter().for_each(|t| visit(t));
                    Some(())
                }
                None => walk(&body, &mut visit),
            };
            if ok.is_none() {
                bad.push(format!("{name} chunk {ci}: malformed front coding"));
                continue;
            }
            // The routing key: empty for chunk 0, else above the previous
            // chunk's last term and at most this chunk's first.
            if let Some(first) = &first {
                if ci == 0 {
                    if !c.key.is_empty() {
                        bad.push(format!("{name} chunk 0: has a routing key"));
                    }
                } else if order.cmp(&before, &c.key) != Ordering::Less
                    || order.cmp(&c.key, first) == Ordering::Greater
                {
                    bad.push(format!(
                        "{name} chunk {ci}: the routing key does not lie between the \
                         neighbouring terms"
                    ));
                }
            }
            prev = Some(last);
        }
        if unsorted > 0 {
            bad.push(format!(
                "{name}: {unsorted} term(s) out of order ({})",
                if order == TermOrder::Sparql {
                    "SPARQL order"
                } else {
                    "byte order"
                }
            ));
        }
        if seen != d.term_count as u64 {
            bad.push(format!(
                "{name}: the root says {} term(s), the chunks hold {seen}",
                d.term_count
            ));
        }
        if order == TermOrder::Sparql && counts != d.groups {
            bad.push(format!(
                "{name}: the root's group counts {:?} are not the terms' {:?}",
                d.groups.0, counts.0
            ));
        }
        total += seen;
    }
    Ok((total, bad))
}
