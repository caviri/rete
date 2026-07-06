// Embed every <key>_texts.json -> <key>_emb.f32 + <key>_emb_index.json, loading
// the model ONCE (multilingual-e5-small q8). Resumable (skips existing emb).
import { pipeline } from "@huggingface/transformers";
import fs from "fs";

const files = fs.readdirSync(".").filter((f) => f.endsWith("_texts.json"));
console.log(`${files.length} corpora to embed`);
const ex = await pipeline("feature-extraction", "Xenova/multilingual-e5-small", { dtype: "q8" });
const dim = 384;

for (const f of files) {
  const name = f.replace("_texts.json", "");
  if (fs.existsSync(`${name}_emb.f32`)) { console.log(`  skip ${name}`); continue; }
  const docs = JSON.parse(fs.readFileSync(f, "utf8"));
  const buf = Buffer.alloc(docs.length * dim * 4);
  const t0 = Date.now();
  for (let i = 0; i < docs.length; i += 64) {
    const batch = docs.slice(i, i + 64).map((d) => "passage: " + d.text);
    const r = await ex(batch, { pooling: "mean", normalize: true });
    for (let j = 0; j < batch.length; j++)
      for (let k = 0; k < dim; k++) buf.writeFloatLE(r.data[j * dim + k], ((i + j) * dim + k) * 4);
  }
  fs.writeFileSync(`${name}_emb.f32`, buf);
  fs.writeFileSync(`${name}_emb_index.json`, JSON.stringify(docs.map((d) => ({ iri: d.iri, title: d.title }))));
  console.log(`  ${name}: ${docs.length} docs, ${(buf.length / 1048576).toFixed(1)} MB, ${((Date.now() - t0) / 1000).toFixed(0)}s`);
}
console.log("EMBED_ALL_DONE");
