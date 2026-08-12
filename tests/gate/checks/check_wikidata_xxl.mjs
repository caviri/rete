import { launchBrowser } from "./_browser.mjs";
import {
  canonicalizeTableText,
  sha256Text,
  validateLiveEvidence,
  validateObjectPins,
  validateShardTraffic,
  writeExclusiveJsonReport,
} from "./wikidata_xxl_traffic.mjs";


const PORT = process.env.PGPORT || "8090";
const OPEN_TIMEOUT_MS = Number(process.env.RETE_OPEN_TIMEOUT_MS || 120000);
const SHARDS = [
  { url: "https://data.graphplaza.com/wikidata-xxl/shard_0000.rete", length: 949270267, etag: '"c4b8ad492c00fc88f44cd4bcd505f25f-15"' },
  { url: "https://data.graphplaza.com/wikidata-xxl/shard_0001.rete", length: 554800567, etag: '"0674b3b74fd9a14a7754effbfc929c79-9"' },
  { url: "https://data.graphplaza.com/wikidata-xxl/shard_0002.rete", length: 708499856, etag: '"8b99a440d2f2e505b93b6953a2d98538-11"' },
  { url: "https://data.graphplaza.com/wikidata-xxl/shard_0003.rete", length: 849908311, etag: '"bbcc87cef8be7e23f0929df0c265b41f-13"' },
  { url: "https://data.graphplaza.com/wikidata-xxl/shard_0004.rete", length: 867722504, etag: '"27fad16c0ffd0b813f49d8d512f2cb8b-13"' },
  { url: "https://data.graphplaza.com/wikidata-xxl/shard_0005.rete", length: 942757112, etag: '"66fc645cb02c604146ec9138f1de1eb6-15"' },
];
const ASK = "ASK WHERE { <http://www.wikidata.org/entity/Q42> ?p ?o }";
const SELECT = [
  "SELECT ?p ?o WHERE {",
  "  <http://www.wikidata.org/entity/Q42> ?p ?o",
  "} ORDER BY ?p ?o LIMIT 10",
].join("\n");
const EXPECTED_SELECT_SHA256 = "d79d99c8b992ef900847b75f136ec410c5b36ac87a8cba292540638d21b6f026";


async function probePinnedObjects() {
  return Promise.all(SHARDS.map(async ({ url, length, etag }, shard) => {
    const identity = {
      shard,
      url,
      expected: { length, etag },
      actual: {},
    };
    try {
      const response = await fetch(url, { method: "HEAD", cache: "no-store" });
      identity.actual = {
        status: response.status,
        contentLength: response.headers.get("content-length") || "",
        etag: response.headers.get("etag") || "",
        acceptRanges: response.headers.get("accept-ranges") || "",
      };
    } catch (error) {
      identity.actual = { error: String(error) };
    }
    return identity;
  }));
}


async function runQuery(page, query) {
  const previous = await page.evaluate(() => (
    (document.getElementById("qmeta") || {}).textContent || ""
  ));
  await page.evaluate((text) => {
    window.PlaygroundEditor.setText("q", text);
    document.getElementById("run").click();
  }, query);
  await page.waitForFunction((old) => {
    const qmeta = ((document.getElementById("qmeta") || {}).textContent || "").trim();
    const running = ((document.getElementById("run") || {}).textContent || "").trim();
    return qmeta && qmeta !== old && running !== "Cancel" && !/querying/i.test(qmeta);
  }, previous, { timeout: 300000 });
  const result = await page.evaluate(() => {
    const table = document.querySelector("#out > .tbl table");
    const headers = table
      ? [...table.querySelectorAll("thead th")].map((cell) => (
        cell.querySelector(".th-name")?.getAttribute("title") || cell.textContent || ""
      ))
      : [];
    const tableRows = table
      ? [...table.querySelectorAll("tbody tr")].map((row) => (
        [...row.querySelectorAll("td")].map((cell) => cell.textContent || "")
      ))
      : [];
    return {
      qmeta: ((document.getElementById("qmeta") || {}).textContent || "").trim(),
      error: !!document.querySelector("#out .error-box"),
      rows: tableRows.length,
      table: { headers, rows: tableRows },
    };
  });
  const canonicalTable = canonicalizeTableText(result.table);
  return { ...result, sha256: sha256Text(canonicalTable) };
}


const objectPinsBefore = await probePinnedObjects();
const pinErrorsBefore = validateObjectPins(objectPinsBefore)
  .map((error) => `before: ${error}`);
if (pinErrorsBefore.length > 0) {
  const report = {
    verdict: "FAIL",
    objectPinsBefore,
    objectPinsAfter: null,
    evidenceErrors: pinErrorsBefore,
  };
  writeExclusiveJsonReport(process.env.RETE_REPORT_PATH, report);
  console.log(JSON.stringify(report, null, 2));
  process.exit(1);
}

const browser = await launchBrowser();
const context = await browser.newContext();
await context.addInitScript(() => localStorage.setItem("asyncReadsOn", "0"));
const page = await context.newPage();
const events = [];
const pageErrors = [];
const requestFailures = [];
page.on("pageerror", (error) => pageErrors.push(String(error).slice(0, 240)));
page.on("requestfailed", (request) => requestFailures.push({
  url: request.url(),
  error: request.failure()?.errorText || "request failed",
}));
page.on("response", (response) => {
  const shard = SHARDS.findIndex(({ url }) => response.url().split("?")[0] === url);
  if (shard < 0) return;
  const request = response.request();
  const headers = response.headers();
  events.push({
    shard,
    method: request.method(),
    status: response.status(),
    range: request.headers().range || "",
    bytes: Number(headers["content-length"] || 0),
    length: SHARDS[shard].length,
  });
});

let ask;
let select;
let shardChip = "";
try {
  await page.goto(
    `http://127.0.0.1:${PORT}/playground.html?task5-wikidata-xxl=1`
      + "#dataset=wikidata-xxl&load=lazy&mode=sparql",
    { waitUntil: "domcontentloaded" },
  );
  try {
    await page.waitForFunction(() => (
      window.PlaygroundEditor
        && document.getElementById("run")
        && document.querySelectorAll("#examples [data-example]").length > 0
    ), undefined, { timeout: OPEN_TIMEOUT_MS });
  } catch (error) {
    const state = await page.evaluate(() => ({
      href: location.href,
      title: document.title,
      readyState: document.readyState,
      hasEditor: !!window.PlaygroundEditor,
      hasRun: !!document.getElementById("run"),
      examples: document.querySelectorAll("#examples [data-example]").length,
      dataset: document.getElementById("dataset")?.value || "",
      out: (document.getElementById("out")?.textContent || "").trim().slice(0, 3000),
    }));
    throw new Error(`playground did not initialize: ${error.message}; ${JSON.stringify({ state, pageErrors, requestFailures })}`);
  }
  // Remote-lazy datasets do not open their sources until the first query.
  // The shard chip is therefore asserted after ASK has triggered that open.
  ask = await runQuery(page, ASK);
  await page.waitForFunction(() => (
    [...document.querySelectorAll(".fed-chip-name")]
      .some((element) => /6 shards/.test(element.textContent || ""))
  ), undefined, { timeout: OPEN_TIMEOUT_MS });
  shardChip = await page.evaluate(() => (
    [...document.querySelectorAll(".fed-chip-name")]
      .map((element) => (element.textContent || "").trim())
      .find((text) => /6 shards/.test(text)) || ""
  ));
  select = await runQuery(page, SELECT);
  await page.waitForTimeout(500);
} finally {
  await browser.close();
}

const objectPinsAfter = await probePinnedObjects();
const pinErrorsAfter = validateObjectPins(objectPinsAfter)
  .map((error) => `after: ${error}`);
const pinErrors = [...pinErrorsBefore, ...pinErrorsAfter];
const traffic = validateShardTraffic(events, SHARDS.map(({ length }) => length));
const evidenceErrors = validateLiveEvidence({
  traffic,
  shardChip,
  ask,
  select,
  pageErrors,
  requestFailures,
  pinErrors,
  expectedSelectSha256: EXPECTED_SELECT_SHA256,
});
const report = {
  verdict: evidenceErrors.length === 0 ? "PASS" : "FAIL",
  objectPinsBefore,
  objectPinsAfter,
  expectedSelectSha256: EXPECTED_SELECT_SHA256,
  shardChip,
  ask: { query: ASK, ...ask },
  select: { query: SELECT, ...select },
  traffic,
  pageErrors,
  requestFailures,
  evidenceErrors,
};
writeExclusiveJsonReport(process.env.RETE_REPORT_PATH, report);
console.log(JSON.stringify(report, null, 2));
process.exit(evidenceErrors.length === 0 ? 0 : 1);
