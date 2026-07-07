#!/usr/bin/env python3
"""Python port of scripts/rag_embed_all.mjs for machines without Node.

Embeds every data/rag/<key>_texts.json -> <key>_emb.f32 + <key>_emb_index.json
using the SAME model the playground's Semantic tab uses to embed the query
(Xenova/multilingual-e5-small, the q8 ONNX), so passage and query vectors are
comparable. Mean-pooled over the attention mask, L2-normalised, "passage: "
prefix. Float32 little-endian matrix, N x 384. Resumable (skips existing emb).

  pip install onnxruntime transformers        # tokenizer-only; no torch needed
  python scripts/rag_embed_onnx.py [key ...]  # default: every *_texts.json
"""
import glob
import json
import os
import sys

import numpy as np
import onnxruntime as ort
from huggingface_hub import hf_hub_download
from transformers import AutoTokenizer

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RAG = os.path.join(ROOT, "data", "rag")
REPO = "Xenova/multilingual-e5-small"
DIM = 384
BATCH = 64
# q8 (model_quantized.onnx) matches what the browser query uses and the canonical
# rag_embed_all.mjs; retrieval is identical to fp32 under WASM. Override with RAG_ONNX.
ONNX_FILE = os.environ.get("RAG_ONNX", "onnx/model_quantized.onnx")

print(f"loading {ONNX_FILE} + tokenizer ...", flush=True)
model_path = hf_hub_download(REPO, ONNX_FILE)
tok = AutoTokenizer.from_pretrained(REPO)
sess = ort.InferenceSession(model_path, providers=["CPUExecutionProvider"])
IN = {i.name for i in sess.get_inputs()}


def embed(texts):
    enc = tok(texts, padding=True, truncation=True, max_length=512, return_tensors="np")
    feed = {"input_ids": enc["input_ids"].astype(np.int64),
            "attention_mask": enc["attention_mask"].astype(np.int64)}
    if "token_type_ids" in IN:
        feed["token_type_ids"] = enc.get(
            "token_type_ids", np.zeros_like(enc["input_ids"])).astype(np.int64)
    last = sess.run(None, feed)[0]                       # [B, T, 384]
    mask = enc["attention_mask"][:, :, None].astype(np.float32)
    emb = (last * mask).sum(1) / np.clip(mask.sum(1), 1e-9, None)
    emb /= np.clip(np.linalg.norm(emb, axis=1, keepdims=True), 1e-9, None)
    return emb.astype("<f4")


keys = sys.argv[1:]
files = ([os.path.join(RAG, f"{k}_texts.json") for k in keys] if keys
         else sorted(glob.glob(os.path.join(RAG, "*_texts.json"))))
for f in files:
    key = os.path.basename(f)[:-len("_texts.json")]
    embf = os.path.join(RAG, f"{key}_emb.f32")
    if os.path.exists(embf):
        print(f"  skip {key} (exists)")
        continue
    docs = json.load(open(f, encoding="utf-8"))
    out = np.empty((len(docs), DIM), dtype="<f4")
    for i in range(0, len(docs), BATCH):
        batch = ["passage: " + d["text"] for d in docs[i:i + BATCH]]
        out[i:i + len(batch)] = embed(batch)
        if i % (BATCH * 20) == 0:
            print(f"  {key}: {i}/{len(docs)}", flush=True)
    out.tofile(embf)
    json.dump([{"iri": d["iri"], "title": d["title"]} for d in docs],
              open(os.path.join(RAG, f"{key}_emb_index.json"), "w", encoding="utf-8"),
              ensure_ascii=False)
    print(f"  {key}: {len(docs)} docs, {out.nbytes/1048576:.1f} MB")
print("EMBED_ALL_DONE")
