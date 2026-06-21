import os
from playwright.sync_api import sync_playwright

BASE = "http://localhost:8123/experiments/plaza"
SHOTS = os.path.dirname(os.path.abspath(__file__))
errs = []

with sync_playwright() as pw:
    b = pw.firefox.launch()
    pg = b.new_page(viewport={"width": 1320, "height": 1000})
    pg.on("pageerror", lambda e: errs.append("PAGEERROR: " + str(e)))
    pg.on("console", lambda m: errs.append(m.text) if m.type == "error" else None)
    pg.goto(BASE + "/index.html", wait_until="load", timeout=60000)
    pg.wait_for_selector(".ocard", timeout=60000)
    pg.wait_for_timeout(3500)
    ocards = pg.eval_on_selector_all(".ocard", "e=>e.length")
    names = pg.eval_on_selector_all(".ocard .ocard-name", "els=>els.slice(0,12).map(e=>e.textContent)")
    print(f"ontology cards = {ocards}")
    print("first names:", names)
    pg.screenshot(path=os.path.join(SHOTS, "onto-section.png"), full_page=True)
    # open a modal — click the ChEBI card if present else the first
    target = ".ocard"
    for i, n in enumerate(names):
        if "ChEBI" == n:
            target = f".ocard >> nth={i}"
            break
    pg.click(target)
    pg.wait_for_selector("#modal:not([hidden]) .modal-used", timeout=10000)
    title = pg.inner_text("#modal h2")
    used = pg.eval_on_selector_all("#modal .modal-used li", "e=>e.length")
    desc = (pg.inner_text("#modal .modal-desc") or "")[:60] if pg.query_selector("#modal .modal-desc") else "(none)"
    print(f"MODAL: title='{title}', used-in datasets={used}, desc='{desc}'")
    pg.screenshot(path=os.path.join(SHOTS, "onto-modal.png"))
    b.close()

print(f"=== ERRORS ({len(errs)}) ===")
for e in errs[:15]:
    print(" -", e[:200])
