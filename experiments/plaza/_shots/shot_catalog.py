import os
from playwright.sync_api import sync_playwright

BASE = "http://localhost:8123/experiments/plaza"
SHOTS = os.path.dirname(os.path.abspath(__file__))

with sync_playwright() as pw:
    b = pw.firefox.launch()
    pg = b.new_page(viewport={"width": 1320, "height": 1000})
    pg.goto(BASE + "/index.html", wait_until="load", timeout=60000)
    pg.wait_for_selector(".card .art img", timeout=60000)
    pg.wait_for_timeout(4500)  # let all 10 cards + p5 finish
    pg.screenshot(path=os.path.join(SHOTS, "catalog-parchment.png"), full_page=True)
    pg.click("#themeBtn")
    pg.wait_for_timeout(3500)
    pg.screenshot(path=os.path.join(SHOTS, "catalog-dark.png"), full_page=True)
    b.close()
print("done")
