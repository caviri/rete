#!/usr/bin/env python3
"""Insert the generated flat-companion entries (scripts/comp_catalog.py) into the
catalog `companions` block, right before its closing brace (which sits just before
the `rag:` block). Brace-counting is unsafe here — companion SQL examples contain
`{` `}` in strings — so anchor on the `rag:` block that follows companions."""
import os, subprocess, sys

CAT = "web/playground-src/catalog.js"
src = open(CAT, encoding="utf-8").read()
block = subprocess.run([sys.executable, "scripts/comp_catalog.py"],
                       capture_output=True, text=True, encoding="utf-8").stdout.rstrip("\n")
if not block.strip():
    sys.exit("no companion entries generated")

anchor = src.index("\n  rag: {")               # rag block follows companions
close = src.rindex("\n  },\n", 0, anchor)      # companions' closing brace
new = src[:close] + "\n" + block + src[close:]
open(CAT, "w", encoding="utf-8", newline="\n").write(new)
n = block.count('": {"rete"') + block.count('": {')
print(f"inserted {block.count('parquetDir')} companion entries into catalog.js")
