import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  canonicalizeTableText,
  sha256Text,
  validateLiveEvidence,
  validateObjectPins,
  validateShardTraffic,
  writeExclusiveJsonReport,
} from "./wikidata_xxl_traffic.mjs";


const canonicalTable = canonicalizeTableText({
  headers: [" ?p ", "?o"],
  rows: [[" alpha ", "beta\n gamma"], ["delta", ""]],
});
assert.equal(
  canonicalTable,
  '{"headers":["?p","?o"],"rows":[["alpha","beta gamma"],["delta",""]]}',
);
assert.equal(
  sha256Text(canonicalTable),
  "3cab3688db41e18a5c321b39f4c40c78b81790447be18b6bd9a05ed7a801fdcc",
);


const pins = [1000, 1100, 1200, 1300, 1400, 1500];
const ranged = pins.map((length, shard) => ({
  shard,
  method: "GET",
  status: 206,
  range: "bytes=0-99",
  bytes: 100,
  length,
}));

const valid = validateShardTraffic(ranged, pins);
assert.equal(valid.ok, true);
assert.equal(valid.totalBytes, 600);
assert.deepEqual(valid.shards.map((entry) => entry.rangeGets), [1, 1, 1, 1, 1, 1]);

const fullGet = validateShardTraffic([
  ...ranged.slice(0, 3),
  { shard: 3, method: "GET", status: 200, range: "", bytes: 1300, length: 1300 },
  ...ranged.slice(4),
], pins);
assert.equal(fullGet.ok, false);
assert.match(fullGet.errors.join("\n"), /shard 3.*unranged GET/);

const missing = validateShardTraffic(ranged.slice(0, 5), pins);
assert.equal(missing.ok, false);
assert.match(missing.errors.join("\n"), /shard 5.*no ranged GET/);

const cumulativelyFull = validateShardTraffic([
  ...ranged,
  { shard: 0, method: "GET", status: 206, range: "bytes=100-999", bytes: 900, length: 1000 },
], pins);
assert.equal(cumulativelyFull.ok, false);
assert.match(cumulativelyFull.errors.join("\n"), /shard 0.*fetched 1000.*of 1000/);

const liveEvidence = {
  traffic: valid,
  shardChip: "⛓ 6 shards",
  ask: { qmeta: "ASK true · federated 6 source(s)", error: false },
  select: {
    qmeta: "10 row(s) · federated 6 source(s)",
    error: false,
    rows: 10,
    sha256: "pinned-select-hash",
  },
  expectedSelectSha256: "pinned-select-hash",
  pageErrors: [],
  requestFailures: [],
  pinErrors: [],
};
assert.deepEqual(validateLiveEvidence(liveEvidence), []);
assert.match(
  validateLiveEvidence({
    ...liveEvidence,
    select: { ...liveEvidence.select, rows: 9 },
  }).join("\n"),
  /expected exactly 10 SELECT rows; got 9/,
);
assert.match(
  validateLiveEvidence({
    ...liveEvidence,
    requestFailures: [{ url: "https://data.example/shard.rete", error: "net::ERR_FAILED" }],
  }).join("\n"),
  /request failed.*shard\.rete.*ERR_FAILED/,
);
assert.match(
  validateLiveEvidence({ ...liveEvidence, pinErrors: ["shard 0 ETag mismatch"] }).join("\n"),
  /shard 0 ETag mismatch/,
);
assert.match(
  validateLiveEvidence({
    ...liveEvidence,
    select: { ...liveEvidence.select, sha256: "different" },
  }).join("\n"),
  /SELECT hash mismatch.*pinned-select-hash.*different/,
);

const pinnedObjects = [{
  shard: 0,
  url: "https://data.example/shard-0.rete",
  expected: { length: 1000, etag: '"pin-0"' },
  actual: {
    status: 200,
    contentLength: "1000",
    etag: '"pin-0"',
    acceptRanges: "bytes",
  },
}];
assert.deepEqual(validateObjectPins(pinnedObjects), []);
const badPins = validateObjectPins([{
  ...pinnedObjects[0],
  actual: {
    status: 503,
    contentLength: "999",
    etag: '"different"',
    acceptRanges: "none",
  },
}]);
assert.match(badPins.join("\n"), /HEAD status 503/);
assert.match(badPins.join("\n"), /Content-Length.*expected 1000.*got "999"/);
assert.match(badPins.join("\n"), /ETag.*expected.*pin-0.*got.*different/);
assert.match(badPins.join("\n"), /Accept-Ranges.*expected bytes.*got "none"/);
assert.match(
  validateObjectPins([{
    ...pinnedObjects[0],
    actual: { error: "socket closed" },
  }]).join("\n"),
  /HEAD failed.*socket closed/,
);

const reportDir = fs.mkdtempSync(path.join(os.tmpdir(), "rete-wikidata-report-"));
try {
  const reportPath = path.join(reportDir, "report.json");
  const report = { verdict: "PASS", totalBytes: 600 };
  writeExclusiveJsonReport(reportPath, report);
  assert.equal(
    fs.readFileSync(reportPath, "utf8"),
    '{\n  "verdict": "PASS",\n  "totalBytes": 600\n}\n',
  );
  assert.throws(
    () => writeExclusiveJsonReport(reportPath, report),
    (error) => error?.code === "EEXIST",
  );
  assert.throws(
    () => writeExclusiveJsonReport(path.join(reportDir, "missing", "report.json"), report),
    (error) => error?.code === "ENOENT",
  );
} finally {
  fs.rmSync(reportDir, { recursive: true, force: true });
}

console.log(JSON.stringify({ verdict: "PASS", checks: 17 }));
