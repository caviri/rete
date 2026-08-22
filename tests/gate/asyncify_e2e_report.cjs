const fs = require('node:fs');
const path = require('node:path');


function parseExpectedPin(lengthText, etag) {
  if (lengthText === undefined && etag === undefined) return null;
  if (lengthText === undefined || etag === undefined) {
    throw new Error('set both RETE_EXPECT_LENGTH and RETE_EXPECT_ETAG');
  }
  if (!/^\d+$/.test(lengthText) || !Number.isSafeInteger(Number(lengthText)) || Number(lengthText) <= 0) {
    throw new Error('RETE_EXPECT_LENGTH must be a positive integer');
  }
  if (!etag) throw new Error('RETE_EXPECT_ETAG must be a non-empty ETag');
  return { length: Number(lengthText), etag };
}


function buildResidentReport({
  url,
  query,
  openMs,
  query1Ms,
  query2Ms,
  stats0,
  stats1,
  stats2,
  result,
  resultSha256,
  pinsBefore = null,
  pinsAfter = null,
  pinErrors = [],
}) {
  const query1 = {
    ms: query1Ms,
    bytes: stats1.bytes - stats0.bytes,
    requests: stats1.requests - stats0.requests,
  };
  const query2 = {
    ms: query2Ms,
    bytes: stats2.bytes - stats1.bytes,
    requests: stats2.requests - stats1.requests,
  };
  const residentReuse = query2.bytes === 0 && query2.requests === 0;
  const rowCount = Array.isArray(result.rows)
    ? result.rows.length
    : (Array.isArray(result.triples) ? result.triples.length : (result.kind === 'ask' ? 1 : 0));
  return {
    verdict: residentReuse && pinErrors.length === 0 ? 'PASS' : 'FAIL',
    url,
    query,
    fileLength: stats2.fileLength,
    open: { ms: openMs, bytes: stats0.bytes, requests: stats0.requests },
    query1,
    query2,
    cumulative: { bytes: stats2.bytes, requests: stats2.requests },
    strictSubset: stats2.bytes < stats2.fileLength,
    residentReuse,
    pinsBefore,
    pinsAfter,
    pinErrors,
    result: {
      kind: result.kind,
      vars: result.vars,
      rows: rowCount,
      sha256: resultSha256,
    },
  };
}


function validateRemotePin(pin) {
  const errors = [];
  const actual = pin.actual || {};
  if (actual.error) return [`${pin.url} HEAD failed: ${actual.error}`];
  if (actual.status !== 200) errors.push(`${pin.url} HEAD status ${actual.status}`);
  const contentLength = actual.contentLength || '';
  const parsedLength = /^\d+$/.test(contentLength) ? Number(contentLength) : Number.NaN;
  if (!Number.isSafeInteger(parsedLength) || parsedLength !== pin.expected.length) {
    errors.push(
      `${pin.url} Content-Length mismatch: expected ${pin.expected.length}; got ${JSON.stringify(contentLength)}`,
    );
  }
  if (actual.etag !== pin.expected.etag) {
    errors.push(
      `${pin.url} ETag mismatch: expected ${JSON.stringify(pin.expected.etag)}; got ${JSON.stringify(actual.etag || '')}`,
    );
  }
  const ranges = (actual.acceptRanges || '')
    .split(',')
    .map((value) => value.trim().toLowerCase());
  if (!ranges.includes('bytes')) {
    errors.push(
      `${pin.url} Accept-Ranges mismatch: expected bytes; got ${JSON.stringify(actual.acceptRanges || '')}`,
    );
  }
  return errors;
}


function writeExclusiveJsonReport(reportPath, report) {
  if (!reportPath) return;
  fs.statSync(path.dirname(reportPath));
  fs.writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`, {
    encoding: 'utf8',
    flag: 'wx',
  });
}


module.exports = {
  buildResidentReport,
  parseExpectedPin,
  validateRemotePin,
  writeExclusiveJsonReport,
};
