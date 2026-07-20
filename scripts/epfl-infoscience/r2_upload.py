"""Direct R2 upload (boto3), bypassing uv/venv. Reads ACCESS_KEY_ID /
SECRET_ACCESS_KEY / S3_API_ENDPOINT from .env. Multipart, with progress.

Usage: python scripts/epfl-infoscience/r2_upload.py <local-file> <object-key> [content-type]
Public at https://data.graphplaza.com/<object-key>
"""
import os, sys, threading, boto3
from boto3.s3.transfer import TransferConfig

env = {}
for line in open(os.path.join(os.path.dirname(__file__), "..", "..", ".env"), encoding="utf-8"):
    if "=" in line and not line.strip().startswith("#"):
        k, v = line.split("=", 1)
        env[k.strip()] = v.strip().strip('"').strip("'")

src, key = sys.argv[1], sys.argv[2]
ctype = sys.argv[3] if len(sys.argv) > 3 else "application/octet-stream"
bucket = os.environ.get("RETE_BUCKET", "rete")
size = os.path.getsize(src)

s3 = boto3.client("s3", endpoint_url=env["S3_API_ENDPOINT"],
                  aws_access_key_id=env["ACCESS_KEY_ID"],
                  aws_secret_access_key=env["SECRET_ACCESS_KEY"], region_name="auto")

done = [0]
lock = threading.Lock()


def cb(n):
    with lock:
        done[0] += n
        pct = 100 * done[0] / size
        print(f"\r  {key}: {done[0]/2**20:8.1f}/{size/2**20:.1f} MiB ({pct:5.1f}%)", end="", flush=True)


cfg = TransferConfig(multipart_threshold=64 * 2**20, multipart_chunksize=64 * 2**20,
                     max_concurrency=4, use_threads=True)
s3.upload_file(src, bucket, key, ExtraArgs={"ContentType": ctype}, Config=cfg, Callback=cb)
print(f"\nDONE -> https://data.graphplaza.com/{key}")
