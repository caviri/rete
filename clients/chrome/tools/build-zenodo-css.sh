#!/bin/sh
# Zenodo only (the other targets stay as they are): every class the shared components use, Semantic UI palette.
#   docker run --rm --memory 4g -v <ext>:/ext node:22 sh /ext/tools/build-zenodo-css.sh
set -e
cd /tmp && mkdir -p n && cd n && npm init -y >/dev/null 2>&1 && npm i tailwindcss@3.4.17 postcss@8 >/dev/null 2>&1
cd /ext
RETELENS_TARGET=zenodo /tmp/n/node_modules/.bin/tailwindcss -c tools/tailwind.config.js -i tools/zenodo.in.css -o /tmp/zenodo.raw.css
cp /ext/tools/scope.mjs /tmp/n/ && cd /tmp/n && node scope.mjs /tmp/zenodo.raw.css /ext/content/zenodo.css
cat /ext/tools/zenodo.static.css >> /ext/content/zenodo.css
ls -la /ext/content/zenodo.css
