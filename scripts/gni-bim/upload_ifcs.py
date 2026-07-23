"""Upload all 224 GNI BIM raw IFC files to R2 under gni-bim/ifc/<id>.ifc, where
<id> matches the model id in the graph (f/model_N or p/model_N_kind) so that each
gnibim:Model's gnibim:ifcFile link resolves. One boto3 session, R2 creds from .env.
Runs in a python:3.12-slim container with boto3."""
import os, re, glob, boto3

for line in open(".env", encoding="utf-8").read().splitlines():
    line = line.strip()
    if "=" in line and not line.startswith("#"):
        k, v = line.split("=", 1); k = k.strip()
        if k in ("S3_API_ENDPOINT", "ACCESS_KEY_ID", "SECRET_ACCESS_KEY") and k not in os.environ:
            os.environ[k] = v.strip()

s3 = boto3.client("s3", endpoint_url=os.environ["S3_API_ENDPOINT"],
                  aws_access_key_id=os.environ["ACCESS_KEY_ID"],
                  aws_secret_access_key=os.environ["SECRET_ACCESS_KEY"], region_name="auto")
BUCKET = os.environ.get("RETE_BUCKET", "rete")
ROOT = "data/gni-bim-dataset/raw"

jobs = []
for p in glob.glob(f"{ROOT}/2025_BIMfundamentals/**/model_*.ifc", recursive=True):
    num = re.search(r"model_(\d+)", os.path.basename(p)).group(1)
    jobs.append((p, f"gni-bim/ifc/f/model_{num}.ifc"))
for p in glob.glob(f"{ROOT}/2026_BIMprojects/**/model_*.ifc", recursive=True):
    m = re.match(r"model_(\d+)_(arc|structure)", os.path.basename(p))
    if m:
        jobs.append((p, f"gni-bim/ifc/p/model_{m.group(1)}_{m.group(2)}.ifc"))
jobs.sort(key=lambda x: x[1])
print(f"uploading {len(jobs)} IFC files ({sum(os.path.getsize(p) for p,_ in jobs)/1e9:.2f} GB)...", flush=True)
for i, (p, key) in enumerate(jobs, 1):
    s3.upload_file(p, BUCKET, key, ExtraArgs={"ContentType": "application/octet-stream"})
    if i % 20 == 0 or i == len(jobs):
        print(f"  {i}/{len(jobs)}", flush=True)
print("done")
