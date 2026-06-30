#!/usr/bin/env python3
"""Validate / clean an N-Triples (or N-Quads) stream before `rete build`.

Real-world dumps carry a few malformed lines that abort a strict parser: stray
trailing `>` in an IRI, spaces inside `<...>`, a missing final ` .`, CR/LF noise,
empty/comment lines. This drops (or, with --strict, fails on) bad lines so the
build sees only well-formed triples, and reports what it removed.

It is a pragmatic line filter, NOT a full RDF parser — it catches the common
breakages that crash a build, not every spec subtlety. Always follow it with the
real check: `rete validate <out.nt>`.

Usage:
  python nt_clean.py raw.nt            > clean.nt      # drop bad lines, report to stderr
  python nt_clean.py --strict raw.nt   > /dev/null     # exit non-zero on the first bad line
  cat *.nt | python nt_clean.py -      > clean.nt       # stdin
"""
import argparse
import re
import sys

# A term: <IRI> (no whitespace/control/angle inside), a blank node, or a literal
# with optional language tag or ^^<datatype>.
IRI = r'<[^\x00-\x20<>"{}|^`\\]*>'
BNODE = r'_:[A-Za-z0-9_][A-Za-z0-9_.-]*'
STRING = r'"(?:[^"\\]|\\.)*"'
LITERAL = rf'{STRING}(?:@[A-Za-z]+(?:-[A-Za-z0-9]+)*|\^\^{IRI})?'
SUBJ = rf'(?:{IRI}|{BNODE})'
PRED = IRI
OBJ = rf'(?:{IRI}|{BNODE}|{LITERAL})'
GRAPH = rf'(?:{IRI}|{BNODE})'                      # N-Quads optional 4th term
TRIPLE = re.compile(rf'^{SUBJ}\s+{PRED}\s+{OBJ}(?:\s+{GRAPH})?\s*\.\s*$')


def main():
    ap = argparse.ArgumentParser(description="Validate/clean N-Triples/N-Quads.")
    ap.add_argument("input", help="input file, or - for stdin")
    ap.add_argument("--strict", action="store_true",
                    help="exit non-zero on the first malformed line (don't drop)")
    ap.add_argument("--max-report", type=int, default=20,
                    help="how many bad lines to echo to stderr (default 20)")
    args = ap.parse_args()

    fh = sys.stdin if args.input == "-" else open(args.input, encoding="utf-8", errors="replace")
    out = sys.stdout
    kept = dropped = 0
    for n, line in enumerate(fh, 1):
        s = line.rstrip("\r\n")
        if not s or s.lstrip().startswith("#"):
            continue                                  # blank / comment
        if TRIPLE.match(s):
            out.write(s + "\n")
            kept += 1
        else:
            dropped += 1
            if dropped <= args.max_report:
                sys.stderr.write(f"  bad line {n}: {s[:160]}\n")
            if args.strict:
                sys.stderr.write(f"nt_clean: malformed line {n} (--strict)\n")
                sys.exit(2)
    sys.stderr.write(f"nt_clean: kept {kept}, dropped {dropped} malformed line(s)\n")
    if fh is not sys.stdin:
        fh.close()


if __name__ == "__main__":
    main()
