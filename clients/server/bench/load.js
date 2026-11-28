// k6 load against rete-server (SERVER=py) or `rete serve` (SERVER=rust).
// One scenario per run, from the environment: SCENARIO, VUS, DURATION,
// TARGET, TOKEN, QUERIES (queries.json). Each virtual user sends its next
// request as soon as the last one is answered. setup() checks one full answer
// first, so a run that measures errors fails instead of looking fast.
import http from 'k6/http';
import { check, fail } from 'k6';

const TARGET = __ENV.TARGET;
const SERVER = __ENV.SERVER || 'py';
const SCENARIO = __ENV.SCENARIO;
const Q = JSON.parse(__ENV.QUERIES);
const GRAPHS = ['g0', 'g1', 'g2', 'g3'];

export const options = {
  scenarios: {
    // A request still open when the time is up gets 5 s, then counts as
    // stalled (run_ms in the result) instead of stretching the run by 30 s.
    load: { executor: 'constant-vus', vus: Number(__ENV.VUS || 1), duration: __ENV.DURATION || '10s',
      gracefulStop: '5s' },
  },
  discardResponseBodies: true,
  summaryTrendStats: ['med', 'p(95)', 'p(99)', 'max'],
};

function sparql(graph, query) {
  const path = SERVER === 'rust' ? '/sparql' : `/sparql/local/${graph}.rete`;
  return `${TARGET}${path}?query=${encodeURIComponent(query)}`;
}

function request(i, keepBody) {
  // Uncompressed unless the scenario says so: `rete serve` never compresses,
  // and gzip is its own cost (see large_gzip).
  const headers = { 'Accept-Encoding': 'identity' };
  if (__ENV.TOKEN) headers.Authorization = `Bearer ${__ENV.TOKEN}`;
  const params = { headers, responseType: keepBody ? 'text' : 'none' };
  const json = (body) => {
    headers['Content-Type'] = 'application/json';
    return JSON.stringify(body);
  };
  switch (SCENARIO) {
    case 'health':
      return http.get(`${TARGET}/health`, params);
    case 'small':
    case 'large':
    case 'count':
    case 'group':
      return http.get(sparql('g0', Q[SCENARIO]), params);
    case 'spread': // the count query, across four graphs instead of one
      return http.get(sparql(GRAPHS[i % GRAPHS.length], Q.count), params);
    case 'large_gzip':
      headers['Accept-Encoding'] = 'gzip';
      return http.get(sparql('g0', Q.large), params);
    case 'api':
      return http.post(`${TARGET}/api/query`, json({ dataset: 'local/g0.rete', query: Q.api }), params);
    case 'mcp':
      headers.Accept = 'application/json, text/event-stream';
      return http.post(`${TARGET}/mcp`, json({
        jsonrpc: '2.0', id: i, method: 'tools/call',
        params: { name: 'sparql_query', arguments: { dataset: 'local/g0.rete', query: Q.api } },
      }), params);
    default:
      fail(`unknown scenario ${SCENARIO}`);
  }
}

export function setup() {
  const r = request(0, true);
  const body = String(r.body || '');
  const answered = SCENARIO === 'health' || body.includes('"bindings"') || body.includes('"table"');
  if (r.status !== 200 || !answered || /"isError":\s*true/.test(body)) {
    fail(`${SERVER} ${SCENARIO}: status ${r.status}: ${body.slice(0, 300)}`);
  }
}

export default function () {
  const r = request(__VU + __ITER, false);
  check(r, { 'status 200': (x) => x.status === 200 });
}

export function handleSummary(data) {
  const m = data.metrics;
  const d = m.http_req_duration.values;
  const out = {
    reqs: m.http_reqs.values.count,
    rps: m.http_reqs.values.rate,
    p50: d.med,
    p95: d['p(95)'],
    p99: d['p(99)'],
    max: d.max,
    failed: m.http_req_failed.values.rate,
    run_ms: data.state.testRunDurationMs,
  };
  return { stdout: `@@RESULT ${JSON.stringify(out)}\n` };
}
