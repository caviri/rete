# Hosting a built site

The output folder is plain files with relative links, so it works at a domain
root or under a sub-path. WebMCP needs a secure context: any `https://` host, or
`http://localhost` while previewing.

## GitHub Pages

```yaml
# .github/workflows/site.yml
on: { push: { branches: [main] } }
permissions: { pages: write, id-token: write, contents: read }
jobs:
  deploy:
    runs-on: ubuntu-latest
    environment: github-pages
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: 22 }
      - run: node path/to/build.mjs site.json --out dist
      - run: node path/to/check.mjs dist
      - uses: actions/upload-pages-artifact@v3
        with: { path: dist }
      - uses: actions/deploy-pages@v4
```

GitHub Pages ignores `_headers`; it already sends `Access-Control-Allow-Origin: *`.
The build writes `.nojekyll` so nothing is filtered out.

## Cloudflare (Pages, or Workers static assets)

`_headers` is read as written: CORS on `api/*` and `llms.txt`, the JSON-LD media
type, revalidation of `assets/*`. Cloudflare joins the values of every rule that
matches a path, so keep one header name in one rule. For Workers static assets:

```jsonc
// wrangler.jsonc
{ "name": "my-site", "compatibility_date": "2026-11-01",
  "assets": { "directory": "./dist", "not_found_handling": "404-page" } }
```

## Netlify

Publish directory `dist`; `_headers` applies as is.

## Hugging Face Spaces (static)

Copy `dist/` into the Space repository root next to a `README.md` with
`sdk: static` in its front matter. Spaces serve `index.html` from the root.

## A bucket (S3, R2, GCS)

Upload the folder, set `index.html` as the index document and `404.html` as the
error document, and give `api/*` a CORS rule allowing `GET` from `*`. Set
`Content-Type` from the extension (`.jsonld` → `application/ld+json`).

## After publishing

Set `baseUrl` in `site.json` to the final address and rebuild: identifiers in the
JSON-LD become absolute and `sitemap.xml` is written. Then run `check.mjs` on the
folder you deploy.
