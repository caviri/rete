#!/bin/sh
# Run in node:22 with the extension mounted at /ext:
#   docker run --rm --memory 4g -v <ext>:/ext node:22 sh /ext/tools/build-css.sh
# Needs tailwindcss@3 + postcss in /tmp/n (npm i inside the container).
set -e
cd /tmp && mkdir -p n && cd n && npm init -y >/dev/null 2>&1 && npm i tailwindcss@3.4.17 postcss@8 >/dev/null 2>&1
cd /ext
# 1. standalone shim: every class my components use (scanned from lib/*.js + viewer.js)
/tmp/n/node_modules/.bin/tailwindcss -c tools/tailwind.config.js -i tools/shim.in.css -o shim.css --minify
# 2. content CSS: only the classes HF's own stylesheet lacks (test/unmatched.json, from test/hf.mjs)
if [ -f test/unmatched.json ]; then
  RETELENS_SAFELIST=/ext/test/unmatched.json /tmp/n/node_modules/.bin/tailwindcss -c tools/tailwind.config.js -i tools/content.in.css -o /tmp/content.raw.css
  cp tools/content.static.css /tmp/ 2>/dev/null || true
  cp /ext/tools/scope.mjs /tmp/n/ && cd /tmp/n && node scope.mjs /tmp/content.raw.css /ext/content/retelens.css
  cat /ext/tools/content.static.css >> /ext/content/retelens.css
fi
# 3. GitHub: every class my components use, palette mapped to Primer variables
cd /ext
RETELENS_TARGET=github /tmp/n/node_modules/.bin/tailwindcss -c tools/tailwind.config.js -i tools/github.in.css -o /tmp/github.raw.css
cp /ext/tools/scope.mjs /tmp/n/ && cd /tmp/n && node scope.mjs /tmp/github.raw.css /ext/content/github.css
cat /ext/tools/github.static.css >> /ext/content/github.css
ls -la /ext/content/github.css
ls -la /ext/shim.css /ext/content/retelens.css 2>/dev/null
