#!/usr/bin/env python3
"""Replace the `rag: { ... }` block in web/playground-src/catalog.js with the
freshly generated one (scripts/rag_catalog.py). Re-run-safe."""
import os, re, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CAT = os.path.join(ROOT, "web", "playground-src", "catalog.js")

block = subprocess.run([sys.executable, os.path.join(ROOT, "scripts", "rag_catalog.py")],
                       capture_output=True, text=True, encoding="utf-8").stdout
block = block.split("\n// ")[0].rstrip() + "\n"          # drop trailing count comment
n = block.count('emb: "https://')

src = open(CAT, encoding="utf-8").read()
new, cnt = re.subn(r'  rag: \{.*?\n  \},\n', block, src, count=1, flags=re.DOTALL)
if cnt != 1:
    print("ERROR: could not locate the rag block"); sys.exit(1)
open(CAT, "w", encoding="utf-8", newline="\n").write(new)
print(f"spliced {n} rag entries into catalog.js")
