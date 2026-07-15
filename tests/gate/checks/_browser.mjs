import { chromium, firefox } from "playwright";


const BROWSERS = { chromium, firefox };
const PREVIEW_DISCOVERY_API = "https://api.github.com/repos/caviri/rete/pulls?*";


async function isolatePreviewDiscovery(target) {
  if (process.env.RETE_LIVE_PREVIEW_DISCOVERY === "1") return target;
  // Keep unrelated browser checks deterministic while check_version_picker
  // supplies its own success and failure responses for this endpoint.
  await target.route(PREVIEW_DISCOVERY_API, (route) => route.fulfill({
    contentType: "application/json",
    body: "[]",
  }));
  return target;
}

export function selectedBrowserName() {
  const name = String(process.env.RETE_BROWSER || "chromium").toLowerCase();
  if (!BROWSERS[name]) {
    throw new Error(`RETE_BROWSER must be chromium or firefox, got ${JSON.stringify(name)}`);
  }
  return name;
}

export async function launchBrowser(options = {}) {
  const browser = await BROWSERS[selectedBrowserName()].launch(options);
  const newContext = browser.newContext.bind(browser);
  const newPage = browser.newPage.bind(browser);

  browser.newContext = async (...args) => isolatePreviewDiscovery(await newContext(...args));
  browser.newPage = async (...args) => isolatePreviewDiscovery(await newPage(...args));
  return browser;
}
