import { chromium, firefox } from "playwright";


const BROWSERS = { chromium, firefox };

export function selectedBrowserName() {
  const name = String(process.env.RETE_BROWSER || "chromium").toLowerCase();
  if (!BROWSERS[name]) {
    throw new Error(`RETE_BROWSER must be chromium or firefox, got ${JSON.stringify(name)}`);
  }
  return name;
}

export function launchBrowser(options = {}) {
  return BROWSERS[selectedBrowserName()].launch(options);
}
