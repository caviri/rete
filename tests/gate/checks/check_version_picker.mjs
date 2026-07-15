import assert from "node:assert/strict";
import { launchBrowser } from "./_browser.mjs";


const PORT = process.env.PGPORT || "8090";
const SHA = "91ac238000000000000000000000000000000000";
const preview = `https://preview.graphplaza.com/pr-72/${SHA}/playground.html`;
const state = "#dataset=bcn&load=lazy&mode=sparql&ex=3";

const browser = await launchBrowser();
let pageErrors = [];
try {
  const context = await browser.newContext();
  const page = await context.newPage();
  page.on("pageerror", (error) => pageErrors.push(String(error)));
  await page.route("https://api.github.com/repos/caviri/rete/pulls?*", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify([
        {
          number: 72,
          title: "Add streaming parser <unsafe>",
          head: { sha: SHA, repo: { full_name: "caviri/rete" } },
        },
        {
          number: 73,
          title: "Fork preview",
          head: {
            sha: "c04d112000000000000000000000000000000000",
            repo: { full_name: "fork/rete" },
          },
        },
      ]),
    });
  });
  await page.route(preview, async (route) => {
    if (route.request().method() === "HEAD") {
      await route.fulfill({ status: 200, body: "" });
    } else {
      await route.fulfill({
        status: 200,
        contentType: "text/html",
        body: "<!doctype html><title>preview destination</title>",
      });
    }
  });

  await page.goto(`http://localhost:${PORT}/playground.html${state}`, {
    waitUntil: "domcontentloaded",
  });
  await page.waitForFunction(
    () => document.querySelectorAll("#versionSelect option").length === 2,
    undefined,
    { timeout: 60000 },
  );
  const labels = await page.locator("#versionSelect option").allTextContents();
  assert.match(labels[0], /^Production/);
  assert.match(labels[1], /PR #72 · Add streaming parser <unsafe> · 91ac238/);
  assert.equal(await page.locator("#versionSelect option option, #versionSelect script").count(), 0);

  await page.selectOption("#versionSelect", preview);
  await page.waitForURL(`${preview}${state}`);
  assert.equal(page.url(), `${preview}${state}`);
  await context.close();

  const fallbackContext = await browser.newContext();
  const fallback = await fallbackContext.newPage();
  fallback.on("pageerror", (error) => pageErrors.push(String(error)));
  await fallback.route("https://api.github.com/repos/caviri/rete/pulls?*", (route) => {
    route.fulfill({ status: 500, contentType: "application/json", body: "{}" });
  });
  await fallback.goto(`http://localhost:${PORT}/playground.html#dataset=scholar&mode=sparql&ex=0`, {
    waitUntil: "domcontentloaded",
  });
  await fallback.waitForFunction(
    () => window.PlaygroundEditor && document.querySelectorAll("#versionSelect option").length === 1,
    undefined,
    { timeout: 60000 },
  );
  assert.equal(await fallback.locator("#out .error-box").count(), 0);
  await fallbackContext.close();

  assert.deepEqual(pageErrors, []);
  console.log(JSON.stringify({
    verdict: "PASS",
    options: labels,
    navigated: `${preview}${state}`,
    fallbackOptions: 1,
  }, null, 2));
} finally {
  await browser.close();
}
