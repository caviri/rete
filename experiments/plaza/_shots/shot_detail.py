import os
from playwright.sync_api import sync_playwright

BASE = "http://localhost:8123/experiments/plaza"
SHOTS = os.path.dirname(os.path.abspath(__file__))

with sync_playwright() as pw:
    b = pw.firefox.launch()
    pg = b.new_page(viewport={"width": 1320, "height": 1000})
    pg.goto(BASE + "/dataset.html?key=chemotion", wait_until="load", timeout=60000)
    pg.wait_for_selector("#schemaGraph svg circle", timeout=45000)
    # ensure parchment (light) theme for the showcase
    if pg.eval_on_selector("html", "e=>e.dataset.theme") != "light":
        pg.click("#themeBtn")
    pg.wait_for_timeout(4500)  # let the force graph settle + hero re-skin
    pg.screenshot(path=os.path.join(SHOTS, "detail-full-parchment.png"), full_page=True)
    b.close()
print("done")
