// The production + PR-preview version selector. Assertions are COLLECTED, not
// thrown (see _expect.mjs): the gate runner reads the last JSON object this
// prints, so a broken option label has to arrive as `{"verdict":"FAIL",
// failures:[…]}` carrying the label it actually found.
import { expect } from "./_expect.mjs";
import { launchBrowser } from "./_browser.mjs";


const PORT = process.env.PGPORT || "8090";
const SHA = "91ac238000000000000000000000000000000000";
const preview = `https://preview.graphplaza.com/pr-72/${SHA}/playground.html`;
const state = "#dataset=bcn&load=lazy&mode=sparql&ex=3";

const t = expect("check_version_picker");
const browser = await launchBrowser();
let pageErrors = [];
let labels = [];
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
  labels = await page.locator("#versionSelect option").allTextContents();
  t.match("productionOption", labels[0], /^Production/);
  t.match("previewOption", labels[1], /PR #72 · Add streaming parser <unsafe> · 91ac238/);
  t.equal("markupInjectedIntoTheSelect",
    await page.locator("#versionSelect option option, #versionSelect script").count(), 0,
    "a PR title must be escaped, never parsed as markup");

  await page.selectOption("#versionSelect", preview);
  await page.waitForURL(`${preview}${state}`);
  t.equal("navigatedUrl", page.url(), `${preview}${state}`);
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
  t.equal("errorBoxesAfterAFailedDiscovery",
    await fallback.locator("#out .error-box").count(), 0,
    "a 500 from the GitHub API must degrade quietly to Production only");
  await fallbackContext.close();

  t.deepEqual("pageErrors", pageErrors, []);
} catch (error) {
  t.threw("version picker", error);
} finally {
  await browser.close();
}

t.finish({
  options: labels,
  navigated: `${preview}${state}`,
  fallbackOptions: 1,
}, { indent: 2 });
