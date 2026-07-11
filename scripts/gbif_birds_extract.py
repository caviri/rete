#!/usr/bin/env python3
"""Extract Spain + Switzerland bird (Aves) occurrences from the keyless GBIF AWS
Open Data Parquet snapshot into local Parquet part-files, via DuckDB httpfs.

RESUMABLE + BATCHED: the snapshot is ~8,500 files / ~285 GB. We list the real
file keys and process them in batches, writing one durable part per batch to
data/gbif_birds/parts/part_NNNNN.parquet. A batch whose part already exists is
skipped, so if the job is killed (e.g. a background-duration limit) just re-run
it — it continues from the last completed part. Each batch is a small, quick,
low-memory COPY.

Source: s3://gbif-open-data-eu-central-1/occurrence/<snapshot>/  (CC-BY-NC).
"""
import os, re, sys, time, urllib.request, duckdb

SNAPSHOT = os.environ.get("GBIF_SNAPSHOT", "2026-07-01")
BATCH = int(os.environ.get("GBIF_BATCH", "400"))       # files per batch
PREFIX = f"occurrence/{SNAPSHOT}/occurrence.parquet/"
HOST = "gbif-open-data-eu-central-1.s3.eu-central-1.amazonaws.com"
OUT_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                       "data", "gbif_birds")
PARTS = os.path.join(OUT_DIR, "parts")
os.makedirs(PARTS, exist_ok=True)


def list_keys():
    keys, tok = [], ""
    while True:
        url = f"https://{HOST}/?list-type=2&prefix={PREFIX}&max-keys=1000"
        if tok:
            url += f"&continuation-token={urllib.request.quote(tok)}"
        x = urllib.request.urlopen(url, timeout=60).read().decode()
        keys += [k for k in re.findall(r"<Key>([^<]+)</Key>", x) if not k.endswith("/")]
        m = re.search(r"<NextContinuationToken>([^<]+)</NextContinuationToken>", x)
        if not m:
            break
        tok = m.group(1)
    return sorted(keys)


def main():
    con = duckdb.connect()
    con.execute("INSTALL httpfs; LOAD httpfs; SET s3_region='eu-central-1';")
    con.execute("SET preserve_insertion_order=false; SET threads=6;")
    keys = list_keys()
    batches = [keys[i:i + BATCH] for i in range(0, len(keys), BATCH)]
    print(f"{len(keys)} files -> {len(batches)} batches of {BATCH}", file=sys.stderr)
    done = 0
    for bi, batch in enumerate(batches):
        part = os.path.join(PARTS, f"part_{bi:05d}.parquet")
        if os.path.exists(part):
            done += 1
            continue
        files = ",".join(f"'s3://{HOST.split('.')[0]}/{k}'" for k in batch)
        tmp = part + ".tmp"
        t = time.time()
        con.execute(f"""
        COPY (
          SELECT gbifid, countrycode,
            kingdom, phylum, class, "order" AS order_, family, genus, species,
            taxonrank, scientificname, taxonkey, specieskey,
            decimallatitude AS lat, decimallongitude AS lon,
            coordinateuncertaintyinmeters AS coord_uncertainty,
            year, month, basisofrecord, datasetkey, institutioncode, individualcount
          FROM read_parquet([{files}])
          WHERE class='Aves' AND countrycode IN ('ES','CH')
            AND decimallatitude IS NOT NULL AND decimallongitude IS NOT NULL
            AND specieskey IS NOT NULL
        ) TO '{tmp}' (FORMAT parquet, COMPRESSION zstd);
        """)
        os.replace(tmp, part)   # atomic: a half-written part never looks done
        done += 1
        print(f"[{done}/{len(batches)}] part_{bi:05d} in {time.time()-t:.0f}s", file=sys.stderr, flush=True)
    # summary
    n = con.execute(f"SELECT COUNT(*) FROM read_parquet('{PARTS}/part_*.parquet')").fetchone()[0]
    by = con.execute(f"SELECT countrycode, COUNT(*) FROM read_parquet('{PARTS}/part_*.parquet') GROUP BY 1 ORDER BY 2 DESC").fetchall()
    sp = con.execute(f"SELECT COUNT(DISTINCT specieskey) FROM read_parquet('{PARTS}/part_*.parquet')").fetchone()[0]
    print(f"DONE: {n:,} occurrences across {len(batches)} parts; by country {dict(by)}; species {sp:,}", file=sys.stderr)


if __name__ == "__main__":
    main()
