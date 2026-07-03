#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""
Upload the Ramón Llull IIIF layer (page images + manifests) to the R2 bucket.
  pages     : pages/<control>/p-NNNN.jpg  ->  ramon_llull/iiif/<control>/p-NNNN.jpg
  manifests : iiif/<control>/manifest.json -> ramon_llull/iiif/<control>/manifest.json
Parallel, resumable (a local .r2done ledger records uploaded keys). Reads R2 creds
from env / .env (ACCESS_KEY_ID, SECRET_ACCESS_KEY, S3_API_ENDPOINT|BUCKET_ENDPOINT).
"""
import os, sys, glob, threading
from concurrent.futures import ThreadPoolExecutor, as_completed
import boto3
from boto3.s3.transfer import TransferConfig

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..",
                                    "data", "bvpb", "ramon_llull"))
BUCKET = os.environ.get("RETE_BUCKET", "rete")
LEDGER = os.path.join(ROOT, ".r2done")
_lock = threading.Lock()

s3 = boto3.client("s3",
    endpoint_url=os.environ.get("S3_API_ENDPOINT") or os.environ["BUCKET_ENDPOINT"],
    aws_access_key_id=os.environ["ACCESS_KEY_ID"],
    aws_secret_access_key=os.environ["SECRET_ACCESS_KEY"], region_name="auto")
_cfg = TransferConfig(use_threads=False)


def load_done():
    if os.path.exists(LEDGER):
        return set(open(LEDGER, encoding="utf-8").read().split("\n"))
    return set()


def jobs(kind):
    out = []
    if kind in ("pages", "all"):
        for f in glob.glob(os.path.join(ROOT, "pages", "*", "p-*.*")):
            if f.endswith((".jpg", ".jpeg", ".png")):
                ctrl = os.path.basename(os.path.dirname(f))
                out.append((f, f"ramon_llull/iiif/{ctrl}/{os.path.basename(f)}", "image/jpeg"))
    if kind in ("manifests", "all"):
        for f in glob.glob(os.path.join(ROOT, "iiif", "*", "manifest.json")):
            ctrl = os.path.basename(os.path.dirname(f))
            out.append((f, f"ramon_llull/iiif/{ctrl}/manifest.json", "application/json"))
    return out


def main():
    kind = sys.argv[1] if len(sys.argv) > 1 else "all"
    done = load_done()
    todo = [j for j in jobs(kind) if j[1] not in done]
    print(f"upload[{kind}]: {len(todo)} objects to {BUCKET} ({len(done)} already done)")
    n = [0]; led = open(LEDGER, "a", encoding="utf-8")

    def put(job):
        local, key, ct = job
        s3.upload_file(local, BUCKET, key, Config=_cfg,
                       ExtraArgs={"ContentType": ct})
        with _lock:
            led.write(key + "\n"); n[0] += 1
            if n[0] % 500 == 0:
                led.flush(); print(f"  {n[0]}/{len(todo)}")
        return key

    with ThreadPoolExecutor(max_workers=32) as ex:
        futs = [ex.submit(put, j) for j in todo]
        for f in as_completed(futs):
            try:
                f.result()
            except Exception as e:
                print("  ! ", e)
    led.flush(); led.close()
    print(f"upload[{kind}]: done, {n[0]} uploaded")


if __name__ == "__main__":
    main()
