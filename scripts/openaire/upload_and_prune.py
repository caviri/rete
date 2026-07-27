"""Upload converted OpenAIRE v11.1.1 tars to the public rete HF bucket, verify,
then delete the local tar — but ONLY for tars whose parquet conversion is done.

Safety rules:
  * a tar is eligible only if the parquet target that consumes it has
    done=True in its _checkpoint.json (never delete un-converted source);
  * upload, then re-list the remote and require an exact byte-size match
    before deleting the local file;
  * if the remote already matches (idempotent re-run), skip the upload and
    just delete the local copy.

Not-yet-converted tars are reported as "pending" and left in place, so this
can be re-run as the conversion finishes each remaining table.

Usage:
  python scripts/openaire/upload_and_prune.py            # eligible tars
  python scripts/openaire/upload_and_prune.py --dry-run  # show plan only
"""

import argparse
import fnmatch
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from tars_to_parquet_2026 import TARGETS, natural_key  # noqa: E402

BASE = r"D:\pro\rete\data\openaire\2026"
OUT_BASE = BASE
DEST = "hf://buckets/katospiegel/rete-public/sources/openaire/2026-v11.1.1"


def target_of(tarname):
    """Which parquet target consumes this tar (or None)."""
    for target, (patterns, *_rest) in TARGETS.items():
        if any(fnmatch.fnmatch(tarname, p) for p in patterns):
            return target
    return None


def is_done(target):
    cp = os.path.join(OUT_BASE, TARGETS[target][1], "_checkpoint.json")
    if not os.path.exists(cp):
        return False
    with open(cp) as f:
        return bool(json.load(f).get("done"))


def remote_sizes():
    """name -> size for everything already in the dest folder."""
    out = subprocess.run(["hf", "buckets", "ls", DEST + "/"],
                         capture_output=True, text=True)
    sizes = {}
    for line in out.stdout.splitlines():
        parts = line.split()
        if len(parts) >= 4 and parts[0].isdigit():
            sizes[os.path.basename(parts[-1])] = int(parts[0])
    return sizes


def upload(local, name):
    r = subprocess.run(["hf", "buckets", "cp", local, f"{DEST}/{name}"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print(f"    upload FAILED: {r.stderr.strip()[:200]}")
    return r.returncode == 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    tars = sorted((f for f in os.listdir(BASE) if f.endswith(".tar")), key=natural_key)
    remote = remote_sizes()
    uploaded = pruned = pending = skipped = 0
    freed = 0

    for name in tars:
        local = os.path.join(BASE, name)
        size = os.path.getsize(local)
        target = target_of(name)
        if target is None:
            print(f"[skip ] {name}: no target mapping")
            skipped += 1
            continue
        if not is_done(target):
            print(f"[wait ] {name}: {target} conversion not done — leaving in place")
            pending += 1
            continue

        if remote.get(name) == size:
            print(f"[have ] {name}: already on HF ({size:,} B), deleting local")
        else:
            if args.dry_run:
                print(f"[plan ] {name}: would upload ({size:,} B) then delete")
                continue
            print(f"[up   ] {name}: uploading {size:,} B …", flush=True)
            if not upload(local, name):
                continue
            if remote_sizes().get(name) != size:
                print(f"    verify FAILED for {name}; NOT deleting")
                continue
            uploaded += 1
            print(f"    verified on HF")

        if args.dry_run:
            continue
        os.remove(local)
        pruned += 1
        freed += size

    print(f"\nuploaded {uploaded}, pruned {pruned} ({freed/2**30:.1f} GiB freed), "
          f"pending {pending}, skipped {skipped}")


if __name__ == "__main__":
    main()
