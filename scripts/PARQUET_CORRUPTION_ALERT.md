# ⚠️ Parquet corruption alert — verify your dataset's Parquet files

**Date:** 2026-07-24 · **Scope:** any Parquet written to the Windows `D:` bind
mount during the multi-session download window of 2026-07-23/24.

## What happened

While ~5+ dataset containers were writing large Parquet files to the same Docker
**Windows bind mount** (`D:\pro\rete`) concurrently, one file
(`data/deps-dev/raw/Projects.parquet`) was **silently corrupted**: 1,727 of its
3,085 row groups failed to decode — `Corrupt snappy compressed data` and
`Couldn't deserialize thrift` errors. Only ~2.25M of 5.12M rows were readable.

The other four deps-dev files were fine, so it's sporadic — but it hit one file
badly, and the trigger (heavy concurrent bind-mount writes) applied to **every**
dataset downloading at the same time.

## Why size + SHA did NOT catch it (the dangerous part)

The corruption is **intra-file** — bad compressed pages inside otherwise-valid
Parquet — not a truncation. So:

- the file's **byte length still matches** what was written,
- its **SHA256 still matches** (the corrupt bytes hash consistently),
- it uploaded to R2 and "verified" byte-for-byte — **but the R2 copy is equally
  corrupt** (it's byte-identical to a corrupt local file).

`count(*)` and reading the schema also still work (they use the footer/metadata).
**Only decoding every row group reveals it.**

## Detect it (do this on your dataset)

Run the full-decode checker on your dataset's Parquet directory:

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w python:3.12-slim \
  bash -lc 'pip install -q pyarrow && python /w/scripts/verify_parquet.py data/<your-dataset>'
```

It prints `clean` / `CORRUPT (n/N row groups bad)` per file and exits non-zero if
anything is corrupt. (No path arg = scan all of `data/`.)

## Fix it

For each `CORRUPT` file:

1. **Delete** the corrupt file **and its `.done`/marker** (so your pipeline
   re-generates it instead of skipping):
   `rm data/<ds>/raw/<file>.parquet data/<ds>/raw/<file>.parquet.done`
2. **Re-generate** it from source — re-run your dataset's download/export for
   that file only (they're all re-derivable from the upstream API/dump/BigQuery).
3. **Re-verify** with `verify_parquet.py` — confirm it now reads clean.
4. If you already **uploaded to R2**, the remote copy is corrupt too — **re-upload
   the clean file** (overwrite), and don't trust the earlier size/SHA "match".

## Prevent recurrence

- **Don't run many large concurrent writes to the same Windows bind mount.**
  Stagger heavy dataset writes, or cap concurrency.
- Prefer writing big outputs to a **Docker named volume** or the container's own
  filesystem, then `docker cp` / move to the bind mount once (single writer).
- **Always full-decode-verify** large Parquet after a Dockerized write on Windows
  — a matching size/hash is NOT sufficient proof of integrity.
