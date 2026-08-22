import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";


export function canonicalizeTableText({ headers, rows }) {
  const normalize = (value) => String(value).replace(/\s+/g, " ").trim();
  return JSON.stringify({
    headers: headers.map(normalize),
    rows: rows.map((row) => row.map(normalize)),
  });
}


export function sha256Text(value) {
  return crypto.createHash("sha256").update(value, "utf8").digest("hex");
}


export function validateShardTraffic(events, pinnedLengths) {
  const errors = [];
  const shards = pinnedLengths.map((length, shard) => ({
    shard,
    length,
    bytes: 0,
    gets: 0,
    rangeGets: 0,
  }));

  for (const event of events) {
    const summary = shards[event.shard];
    if (!summary) {
      errors.push(`unknown shard index ${event.shard}`);
      continue;
    }
    if (event.method !== "GET") continue;
    summary.gets++;
    summary.bytes += event.bytes;
    if (!event.range) {
      errors.push(`shard ${event.shard} issued an unranged GET`);
      continue;
    }
    summary.rangeGets++;
    if (event.status !== 206) {
      errors.push(`shard ${event.shard} ranged GET returned status ${event.status}`);
    }
    if (!Number.isSafeInteger(event.bytes) || event.bytes <= 0) {
      errors.push(`shard ${event.shard} returned invalid byte count ${event.bytes}`);
    }
  }

  for (const summary of shards) {
    if (summary.rangeGets === 0) {
      errors.push(`shard ${summary.shard} had no ranged GET`);
    }
    if (summary.bytes >= summary.length) {
      errors.push(
        `shard ${summary.shard} fetched ${summary.bytes} bytes of ${summary.length}`,
      );
    }
  }

  const totalBytes = shards.reduce((sum, summary) => sum + summary.bytes, 0);
  const totalLength = pinnedLengths.reduce((sum, length) => sum + length, 0);
  if (totalBytes >= totalLength) {
    errors.push(`all shards fetched ${totalBytes} bytes of ${totalLength}`);
  }

  return { ok: errors.length === 0, errors, totalBytes, totalLength, shards };
}


export function validateObjectPins(objects) {
  const errors = [];
  for (const object of objects) {
    const label = `shard ${object.shard} (${object.url})`;
    const actual = object.actual || {};
    if (actual.error) {
      errors.push(`${label} HEAD failed: ${actual.error}`);
      continue;
    }
    if (actual.status !== 200) errors.push(`${label} HEAD status ${actual.status}`);
    const contentLength = actual.contentLength || "";
    const parsedLength = /^\d+$/.test(contentLength) ? Number(contentLength) : Number.NaN;
    if (!Number.isSafeInteger(parsedLength) || parsedLength !== object.expected.length) {
      errors.push(
        `${label} Content-Length mismatch: expected ${object.expected.length}; got ${JSON.stringify(contentLength)}`,
      );
    }
    if (actual.etag !== object.expected.etag) {
      errors.push(
        `${label} ETag mismatch: expected ${JSON.stringify(object.expected.etag)}; got ${JSON.stringify(actual.etag || "")}`,
      );
    }
    const ranges = (actual.acceptRanges || "")
      .split(",")
      .map((value) => value.trim().toLowerCase());
    if (!ranges.includes("bytes")) {
      errors.push(
        `${label} Accept-Ranges mismatch: expected bytes; got ${JSON.stringify(actual.acceptRanges || "")}`,
      );
    }
  }
  return errors;
}


export function validateLiveEvidence({
  traffic,
  shardChip,
  ask,
  select,
  pageErrors,
  requestFailures,
  pinErrors,
  expectedSelectSha256,
}) {
  const errors = [];
  errors.push(...(pinErrors || []));
  if (!traffic?.ok) {
    errors.push(...(traffic?.errors?.length ? traffic.errors : ["traffic validation failed"]));
  }
  if (ask?.error) errors.push("ASK rendered an error result");
  if (!/ASK true/i.test(ask?.qmeta || "")) errors.push("ASK result was not true");
  if (!/federated 6 source\(s\)/i.test(ask?.qmeta || "")) {
    errors.push("ASK did not report six federated sources");
  }
  if (select?.error) errors.push("SELECT rendered an error result");
  if (select?.rows !== 10) {
    errors.push(`expected exactly 10 SELECT rows; got ${select?.rows ?? "missing"}`);
  }
  if (select?.sha256 !== expectedSelectSha256) {
    errors.push(
      `SELECT hash mismatch: expected ${expectedSelectSha256 || "missing"}; got ${select?.sha256 || "missing"}`,
    );
  }
  if (!/federated 6 source\(s\)/i.test(select?.qmeta || "")) {
    errors.push("SELECT did not report six federated sources");
  }
  if (!/6 shards/.test(shardChip || "")) errors.push("six-shard chip was not rendered");
  for (const error of pageErrors || []) errors.push(`page error: ${error}`);
  for (const failure of requestFailures || []) {
    errors.push(`request failed: ${failure.url || "unknown URL"}: ${failure.error || "unknown error"}`);
  }
  return errors;
}


export function writeExclusiveJsonReport(reportPath, report) {
  if (!reportPath) return;
  fs.statSync(path.dirname(reportPath));
  fs.writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`, {
    encoding: "utf8",
    flag: "wx",
  });
}
