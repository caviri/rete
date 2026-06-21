import os
from playwright.sync_api import sync_playwright

BASE = "http://localhost:8123/experiments/plaza"
SHOTS = os.path.dirname(os.path.abspath(__file__))
errors = []

with sync_playwright() as pw:
    b = pw.firefox.launch()
    pg = b.new_page(viewport={"width": 1320, "height": 1000})
    pg.on("pageerror", lambda e: errors.append("PAGEERROR: " + str(e)))
    pg.on("console", lambda m: errors.append("CONSOLE:" + m.text) if m.type == "error" else None)
    pg.goto(BASE + "/dataset.html?key=chebi-full", wait_until="load", timeout=60000)
    pg.wait_for_selector("#tblLoad", timeout=45000)
    if pg.eval_on_selector("html", "e=>e.dataset.theme") != "light":
        pg.click("#themeBtn")
    pg.click("#tblLoad")
    ok = False
    try:
        pg.wait_for_selector("#tblResults table.rs", timeout=90000)  # DuckDB load + manifest query
        ok = True
    except Exception as ex:
        errors.append("tbl-wait: " + str(ex)[:160])
    pg.wait_for_timeout(1200)
    cols = pg.eval_on_selector_all("#tblResults table.rs th", "e=>e.length") if ok else 0
    rows = pg.eval_on_selector_all("#tblResults table.rs tbody tr", "e=>e.length") if ok else 0
    files = pg.eval_on_selector_all("#tblFiles button", "e=>e.length")
    print(f"TABLE EXPLORER chebi-full: results table={ok}, cols={cols}, rows={rows}, table-buttons={files}")
    pg.locator("#tblExplore").screenshot(path=os.path.join(SHOTS, "tables-chebi.png"))
    b.close()

print(f"=== ERRORS ({len(errors)}) ===")
for e in errors[:20]:
    print(" -", e[:240])
