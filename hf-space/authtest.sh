#!/usr/bin/env bash
set -u
pip install -q -r requirements.txt >/dev/null 2>&1
apt-get update -qq >/dev/null 2>&1 && apt-get install -y -qq curl >/dev/null 2>&1
uvicorn app:app --host 127.0.0.1 --port 7860 >/tmp/u.log 2>&1 &
for i in $(seq 1 20); do curl -s -o /dev/null http://127.0.0.1:7860/health && break; sleep 1; done
B=http://127.0.0.1:7860
code(){ curl -s -o /dev/null -w "%{http_code}" "$@"; }
echo "no token        / : $(code $B/)"
echo "no token   range  : $(code -r 0-7 $B/data/wikidata.rete)"
echo "wrong token       : $(code -r 0-7 "$B/data/wikidata.rete?token=bad")"
echo "correct (query)   : $(code -r 0-7 "$B/data/wikidata.rete?token=testsecret123")"
echo "correct (Bearer)  : $(code -H 'Authorization: Bearer testsecret123' -r 0-7 $B/data/wikidata.rete)"
echo "OPTIONS preflight : $(code -X OPTIONS -H 'Origin: https://caviri.github.io' -H 'Access-Control-Request-Method: GET' $B/data/wikidata.rete)"
echo "health (open)     : $(code $B/health)"
