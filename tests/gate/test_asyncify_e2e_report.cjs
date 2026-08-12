const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {
  buildResidentReport,
  parseExpectedPin,
  validateRemotePin,
  writeExclusiveJsonReport,
} = require('./asyncify_e2e_report.cjs');


assert.equal(parseExpectedPin(undefined, undefined), null);
assert.deepEqual(parseExpectedPin('1000', '"pin"'), { length: 1000, etag: '"pin"' });
assert.throws(() => parseExpectedPin('1000', undefined), /set both/);
assert.throws(() => parseExpectedPin(undefined, '"pin"'), /set both/);
assert.throws(() => parseExpectedPin('1.5', '"pin"'), /positive integer/);
assert.throws(() => parseExpectedPin('0', '"pin"'), /positive integer/);
assert.throws(() => parseExpectedPin('1000', ''), /non-empty ETag/);


const report = buildResidentReport({
  url: 'https://data.example/graph.rete',
  query: 'SELECT ?s WHERE { ?s ?p ?o } ORDER BY ?s LIMIT 10',
  openMs: 100,
  query1Ms: 20,
  query2Ms: 1,
  stats0: { fileLength: 1000, bytes: 400, requests: 2 },
  stats1: { fileLength: 1000, bytes: 700, requests: 5 },
  stats2: { fileLength: 1000, bytes: 700, requests: 5 },
  result: { kind: 'select', vars: ['s'], rows: [{ s: '<http://ex/a>' }] },
  resultSha256: 'abc123',
});
assert.deepEqual(report, {
  verdict: 'PASS',
  url: 'https://data.example/graph.rete',
  query: 'SELECT ?s WHERE { ?s ?p ?o } ORDER BY ?s LIMIT 10',
  fileLength: 1000,
  open: { ms: 100, bytes: 400, requests: 2 },
  query1: { ms: 20, bytes: 300, requests: 3 },
  query2: { ms: 1, bytes: 0, requests: 0 },
  cumulative: { bytes: 700, requests: 5 },
  strictSubset: true,
  residentReuse: true,
  pinsBefore: null,
  pinsAfter: null,
  pinErrors: [],
  result: { kind: 'select', vars: ['s'], rows: 1, sha256: 'abc123' },
});

const refetched = buildResidentReport({
  url: 'https://data.example/graph.rete',
  query: 'ASK { ?s ?p ?o }',
  openMs: 100,
  query1Ms: 20,
  query2Ms: 10,
  stats0: { fileLength: 1000, bytes: 400, requests: 2 },
  stats1: { fileLength: 1000, bytes: 700, requests: 5 },
  stats2: { fileLength: 1000, bytes: 800, requests: 6 },
  result: { kind: 'ask', boolean: true },
  resultSha256: 'def456',
});
assert.equal(refetched.verdict, 'FAIL');
assert.equal(refetched.residentReuse, false);
assert.deepEqual(refetched.query2, { ms: 10, bytes: 100, requests: 1 });

const validPin = {
  url: 'https://data.example/graph.rete',
  expected: { length: 1000, etag: '"pin"' },
  actual: {
    status: 200,
    contentLength: '1000',
    etag: '"pin"',
    acceptRanges: 'bytes',
  },
};
assert.deepEqual(validateRemotePin(validPin), []);
const pinErrors = validateRemotePin({
  ...validPin,
  actual: {
    status: 503,
    contentLength: '999',
    etag: '"different"',
    acceptRanges: 'none',
  },
});
assert.match(pinErrors.join('\n'), /HEAD status 503/);
assert.match(pinErrors.join('\n'), /Content-Length.*expected 1000.*got "999"/);
assert.match(pinErrors.join('\n'), /ETag.*expected.*pin.*got.*different/);
assert.match(pinErrors.join('\n'), /Accept-Ranges.*expected bytes.*got "none"/);
assert.match(
  validateRemotePin({ ...validPin, actual: { error: 'socket closed' } }).join('\n'),
  /HEAD failed.*socket closed/,
);

const mutated = buildResidentReport({
  url: 'https://data.example/graph.rete',
  query: 'ASK { ?s ?p ?o }',
  openMs: 100,
  query1Ms: 20,
  query2Ms: 1,
  stats0: { fileLength: 1000, bytes: 400, requests: 2 },
  stats1: { fileLength: 1000, bytes: 700, requests: 5 },
  stats2: { fileLength: 1000, bytes: 700, requests: 5 },
  result: { kind: 'ask', boolean: true },
  resultSha256: 'def456',
  pinsBefore: validPin,
  pinsAfter: { ...validPin, actual: { ...validPin.actual, etag: '"changed"' } },
  pinErrors: ['after: ETag mismatch'],
});
assert.equal(mutated.verdict, 'FAIL');
assert.equal(mutated.residentReuse, true);
assert.deepEqual(mutated.pinErrors, ['after: ETag mismatch']);

const reportDir = fs.mkdtempSync(path.join(os.tmpdir(), 'rete-resident-report-'));
try {
  const reportPath = path.join(reportDir, 'report.json');
  writeExclusiveJsonReport(reportPath, { verdict: 'PASS', residentReuse: true });
  assert.equal(
    fs.readFileSync(reportPath, 'utf8'),
    '{\n  "verdict": "PASS",\n  "residentReuse": true\n}\n',
  );
  assert.throws(
    () => writeExclusiveJsonReport(reportPath, report),
    (error) => error?.code === 'EEXIST',
  );
  assert.throws(
    () => writeExclusiveJsonReport(path.join(reportDir, 'missing', 'report.json'), report),
    (error) => error?.code === 'ENOENT',
  );
} finally {
  fs.rmSync(reportDir, { recursive: true, force: true });
}

console.log(JSON.stringify({ verdict: 'PASS', checks: 16 }));
