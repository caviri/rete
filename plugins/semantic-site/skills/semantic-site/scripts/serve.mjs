#!/usr/bin/env node
// A static file server for previewing a built site. WebMCP needs a secure
// context, and http://localhost counts as one, so this is enough to try the
// tools in a browser.
//
//   node serve.mjs <dir> [--port 8080] [--host 127.0.0.1]

import http from 'node:http';
import { createReadStream, existsSync, statSync } from 'node:fs';
import { resolve, join, extname, sep } from 'node:path';

const TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.jsonld': 'application/ld+json; charset=utf-8',
  '.txt': 'text/plain; charset=utf-8',
  '.md': 'text/markdown; charset=utf-8',
  '.xml': 'application/xml; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
  '.gif': 'image/gif',
  '.ico': 'image/x-icon',
  '.pdf': 'application/pdf',
  '.woff2': 'font/woff2',
  '.wasm': 'application/wasm',
};

const args = process.argv.slice(2);
const opt = (name, def) => {
  const i = args.indexOf(name);
  return i >= 0 ? args.splice(i, 2)[1] : def;
};
const port = Number(opt('--port', 8080));
const host = opt('--host', '127.0.0.1');
const root = resolve(args[0] || 'dist');
if (!existsSync(root) || !statSync(root).isDirectory()) {
  console.error(`serve: ${root} is not a directory`);
  process.exit(1);
}

function send(res, status, file, headers = {}) {
  res.writeHead(status, {
    'Content-Type': TYPES[extname(file).toLowerCase()] || 'application/octet-stream',
    'Cache-Control': 'no-store',
    ...headers,
  });
  createReadStream(file).pipe(res);
}

http.createServer((req, res) => {
  let path;
  try {
    path = decodeURIComponent(new URL(req.url, 'http://x').pathname);
  } catch (_) {
    res.writeHead(400).end('bad request');
    return;
  }
  const file = resolve(join(root, path));
  if (file !== root && !file.startsWith(root + sep)) {
    res.writeHead(403).end('forbidden');
    return;
  }
  const cors = path.startsWith('/api/') || path === '/llms.txt' ? { 'Access-Control-Allow-Origin': '*' } : {};
  if (existsSync(file) && statSync(file).isDirectory()) {
    if (!path.endsWith('/')) {
      res.writeHead(301, { Location: `${path}/` }).end();
      return;
    }
    const index = join(file, 'index.html');
    if (existsSync(index)) return send(res, 200, index, cors);
  } else if (existsSync(file)) {
    return send(res, 200, file, cors);
  }
  const nf = join(root, '404.html');
  if (existsSync(nf)) return send(res, 404, nf);
  res.writeHead(404, { 'Content-Type': 'text/plain' }).end('not found');
}).listen(port, host, () => {
  console.log(`Serving ${root} at http://${host === '0.0.0.0' ? 'localhost' : host}:${port}/`);
});
