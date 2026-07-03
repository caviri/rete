#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""
Post-correct early-modern Latin OCR: the dominant error is the long-s (ſ) being
read as 'f' (fapientia -> sapientia). A naive ſ->s replace can't fix it because
Tesseract emits 'f', not 'ſ'. So we do DICTIONARY-GUIDED repair: for each token,
try flipping f<->s (and fixing ligatures / stray marks) and keep the variant that
is a real Latin word, per a lexicon built from Whitaker's Words + Collatinus.

Also exposes latin_hit_rate() — the share of tokens that are valid Latin words —
a lexicon-grounded quality metric for before/after comparison.
"""
import os, re, unicodedata, itertools, functools

MODELS = os.path.join(os.path.dirname(__file__), "..", "data", "bvpb",
                      "ramon_llull", "ocr_models")
LIG = {"ﬁ": "fi", "ﬂ": "fl", "ﬀ": "ff", "ﬃ": "ffi", "ﬄ": "ffl", "ﬆ": "st",
       "æ": "ae", "Æ": "Ae", "œ": "oe", "Œ": "Oe", "�": ""}
MINPREF = 4


def _norm(w):
    """Fold for lexicon lookup: lowercase, drop diacritics/macra, v->u, j->i."""
    w = unicodedata.normalize("NFKD", w)
    w = "".join(c for c in w if not unicodedata.combining(c)).lower()
    w = re.sub(r"[^a-z]", "", w).replace("v", "u").replace("j", "i")
    return w


@functools.lru_cache(maxsize=1)
def load_lexicon():
    prefixes, words = set(), set()
    dl = os.path.join(MODELS, "dictline.gen")
    if os.path.exists(dl):
        for line in open(dl, encoding="latin-1"):
            for tok in re.findall(r"[A-Za-z]+", line[:76]):
                s = _norm(tok)
                if len(s) >= 3 and s != "zzz":
                    words.add(s)
                    if len(s) >= MINPREF:
                        prefixes.add(s)
    lm = os.path.join(MODELS, "lemmes.la")
    if os.path.exists(lm):
        for line in open(lm, encoding="utf-8", errors="replace"):
            if line.startswith("!") or "|" not in line:
                continue
            s = _norm(line.split("|", 1)[0])
            if len(s) >= 3:
                words.add(s)
                if len(s) >= MINPREF:
                    prefixes.add(s)
    return prefixes, words


def is_latin(tok):
    pref, words = load_lexicon()
    s = _norm(tok)
    if len(s) < 2:
        return False
    if s in words:
        return True
    for L in range(len(s), MINPREF - 1, -1):     # begins with a known stem?
        if s[:L] in pref:
            return True
    return False


def _prenorm(tok):
    for a, b in LIG.items():
        tok = tok.replace(a, b)
    return tok.replace("ſ", "s")


_FLIP = {"f": "s", "F": "S", "s": "f", "S": "F"}


def fix_token(tok):
    """Return (fixed_token, changed?). Dictionary-guided f<->s repair: among the
    base token and its s/f-flip variants, choose the one that is a valid Latin
    word with the FEWEST remaining 'f' (real Latin rarely has f; long-s misreads
    add spurious f's), tie-broken by fewest edits from the base."""
    base = _prenorm(tok)
    if not re.search(r"[A-Za-z]", base):
        return base, base != tok
    pos = [i for i, c in enumerate(base) if c in "sSfF"]
    if not pos:
        return base, base != tok
    if len(pos) > 8:                                  # keep the search bounded
        pos = [i for i in pos if base[i] in "fF"][:8]

    def score(cand):
        return (is_latin(cand), -_norm(cand).count("f"))

    best, best_sc, best_edits = base, score(base), 0
    for r in range(1, len(pos) + 1):
        for sub in itertools.combinations(pos, r):
            cand = list(base)
            for i in sub:
                cand[i] = _FLIP[cand[i]]
            cand = "".join(cand)
            sc = score(cand)
            if sc > best_sc or (sc == best_sc and r < best_edits):
                best, best_sc, best_edits = cand, sc, r
    return best, best != tok


def fix_words(words):
    """words: list of {'t':token,...}; returns (new_list, n_changed)."""
    out, n = [], 0
    for w in words:
        ft, ch = fix_token(w["t"])
        d = dict(w); d["t"] = ft; d["raw"] = w["t"]
        n += 1 if ch else 0
        out.append(d)
    return out, n


def latin_hit_rate(words):
    toks = [w["t"] for w in words if len(_norm(w["t"])) >= 2]
    if not toks:
        return 0.0, 0, 0
    hits = sum(1 for t in toks if is_latin(t))
    return 100.0 * hits / len(toks), hits, len(toks)


if __name__ == "__main__":
    import sys, json
    sys.stdout.reconfigure(encoding="utf-8")
    pref, words = load_lexicon()
    print(f"lexicon: {len(words)} words / {len(pref)} prefixes")
    for t in sys.argv[1:] or ["fapientia", "philofophiæ", "illuftriffimo", "eſſe", "regi"]:
        ft, ch = fix_token(t)
        print(f"  {t!r:20} -> {ft!r:20} latin={is_latin(ft)} changed={ch}")
