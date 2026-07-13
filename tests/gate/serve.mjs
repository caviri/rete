// Static file server for the docs/ directory, with HTTP Range (206) support so
// the explorer's range reader can fault byte ranges exactly as GitHub Pages
// serves them. Used by the Playwright tests.
import { createServer } from "node:http";
import { open, stat } from "node:fs/promises";
import { extname, join, normalize } from "node:path";

const ROOT = process.argv[2] || "/work/docs";
const PORT = +(process.argv[3] || 8080);
const MIME = {
  ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript",
  ".wasm": "application/wasm", ".json": "application/json", ".css": "text/css",
  ".rete": "application/octet-stream", ".parquet": "application/octet-stream",
};

createServer(async (req, res) => {
  let fh;
  try {
    let p = decodeURIComponent(new URL(req.url, "http://x").pathname);
    if (p === "/") p = "/index.html";
    const file = join(ROOT, normalize(p).replace(/^(\.\.[/\\])+/, ""));
    const size = (await stat(file)).size;
    const ctype = MIME[extname(file)] || "application/octet-stream";
    const range = req.headers.range && /bytes=(\d+)-(\d*)/.exec(req.headers.range);
    fh = await open(file, "r");
    if (range) {
      const start = +range[1];
      const end = range[2] ? Math.min(+range[2], size - 1) : size - 1;
      const len = end - start + 1;
      const buf = Buffer.allocUnsafe(len);
      await fh.read(buf, 0, len, start);
      res.writeHead(206, {
        "Content-Type": ctype, "Content-Range": `bytes ${start}-${end}/${size}`,
        "Accept-Ranges": "bytes", "Content-Length": len, "Service-Worker-Allowed": "/",
      });
      res.end(buf);
    } else {
      const buf = Buffer.allocUnsafe(size);
      await fh.read(buf, 0, size, 0);
      res.writeHead(200, { "Content-Type": ctype, "Accept-Ranges": "bytes", "Service-Worker-Allowed": "/" });
      res.end(buf);
    }
  } catch {
    res.writeHead(404); res.end("not found");
  } finally {
    if (fh) await fh.close();
  }
}).listen(PORT, () => console.log(`serving ${ROOT} on http://localhost:${PORT} (Range-capable)`));
