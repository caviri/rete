#!/usr/bin/env bash
# Watchdog + finisher for the BVPB OCR pass. Waits until all books are OCR'd
# (restarting the resumable container if it dies), then extracts the per-book hocr
# checkpoints, builds the IIIF manifests + full text, and the graph supplement.
set -u
cd /d/pro/rete
export MSYS_NO_PATHCONV=1
IO="D:/pro/rete/data/bvpb/ramon_llull/ocr_io"
HOCR="data/bvpb/ramon_llull/ocr_io/hocr"
NB=$(sed 's|/.*||' "$IO/list.txt" | sort -u | wc -l)
echo "watchdog: target $NB books"

while :; do
  n=$(ls "$HOCR"/*.tgz 2>/dev/null | wc -l)
  c=$(docker ps -q --filter ancestor=rete-ocr:latest | wc -l)
  echo "watch: $n/$NB checkpoints, container=$c"
  [ "$n" -ge "$NB" ] && break
  if [ "$c" -eq 0 ]; then
    echo "watch: OCR container down at $n — restarting (resumable)"
    docker run -d -e OMP_THREAD_LIMIT=1 -v "$IO:/io" rete-ocr:latest /ocr_r2.sh >/dev/null
  fi
  sleep 15
done
echo "watch: all $NB books OCR'd"

# extract per-book hocr checkpoints into pages/
cd data/bvpb/ramon_llull/pages
for t in ../ocr_io/hocr/*.tgz; do tar -xzf "$t" 2>/dev/null; done
cd /d/pro/rete
echo "extracted hocr on disk: $(ls data/bvpb/ramon_llull/pages/*/*.hocr 2>/dev/null | wc -l)"

python scripts/bvpb_ocr_iiif.py build
python scripts/bvpb_iiif_to_nt.py
echo "FINISH DONE"
