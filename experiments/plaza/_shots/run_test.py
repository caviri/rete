import os
from playwright.sync_api import sync_playwright

BASE = "http://localhost:8123/experiments/plaza"
SHOTS = os.path.dirname(os.path.abspath(__file__))
errors, warns = [], []


def hook(pg):
    pg.on("console", lambda m: (errors if m.type == "error" else warns).append(m.text))
    pg.on("pageerror", lambda e: errors.append("PAGEERROR: " + str(e)))


with sync_playwright() as pw:
    b = pw.firefox.launch()
    pg = b.new_page(viewport={"width": 1280, "height": 900})
    hook(pg)

    # --- index (gallery) ---
    pg.goto(BASE + "/index.html", wait_until="load", timeout=60000)
    pg.wait_for_selector(".card .art img", timeout=60000)
    pg.wait_for_timeout(4000)  # let remote cards + p5 finish
    imgs = pg.eval_on_selector_all(".card .art img", "els=>els.map(e=>(e.getAttribute('src')||'').slice(0,11))")
    data_imgs = [s for s in imgs if s.startswith("data:image")]
    print(f"INDEX: {len(imgs)} tile imgs, {len(data_imgs)} are data-URL (p5 rendered)")
    pg.screenshot(path=os.path.join(SHOTS, "index-dark.png"))

    pg.click("#themeBtn")
    pg.wait_for_timeout(3000)
    pg.screenshot(path=os.path.join(SHOTS, "index-light.png"))

    # --- detail: chemotion (has card + schema) ---
    pg.goto(BASE + "/dataset.html?key=chemotion", wait_until="load", timeout=60000)
    boxes = 0
    try:
        pg.wait_for_selector("#schemaGraph svg .ubox", timeout=60000)
        boxes = pg.eval_on_selector_all("#schemaGraph svg .ubox", "e=>e.length")
    except Exception as ex:
        errors.append("schema-wait: " + str(ex)[:160])
    edges = pg.eval_on_selector_all("#schemaGraph svg .uedge", "e=>e.length")
    print(f"DETAIL chemotion: UML boxes={boxes}, edges={edges}")
    hero_el = pg.query_selector(".detail-hero .art img")
    hero = (hero_el.get_attribute("src") or "")[:11] if hero_el else "(none)"
    fdd = bool(pg.query_selector(".files-dd summary"))
    copy = bool(pg.query_selector("#copyRete"))
    print(f"DETAIL chemotion: hero={hero}, filesDropdown={fdd}, copyBtn={copy}")

    # exercise the schema graph: hover the first node, check the info panel fills
    info_before = (pg.inner_text("#schemaInfo") or "")[:30]
    try:
        pg.hover("#schemaGraph svg .ubox")
        pg.wait_for_timeout(800)
    except Exception as ex:
        errors.append("hover: " + str(ex)[:120])
    info_after = (pg.inner_text("#schemaInfo") or "")[:30]
    print(f"DETAIL schemaInfo before='{info_before}' after-hover='{info_after}'")

    pg.locator(".detail-hero .art").screenshot(path=os.path.join(SHOTS, "hero-chemotion.png"))
    # also grab the parchment (light) version of the hero plate
    if pg.eval_on_selector("html", "e=>e.dataset.theme") != "light":
        pg.click("#themeBtn"); pg.wait_for_timeout(1800)
    pg.locator(".detail-hero .art").screenshot(path=os.path.join(SHOTS, "hero-chemotion-light.png"))
    pg.locator("#schemaGraph").screenshot(path=os.path.join(SHOTS, "schema-zoom.png"))
    # click a box to expand its literal properties
    try:
        pg.click("#schemaGraph .ubox")
        pg.wait_for_timeout(2000)
    except Exception as ex:
        errors.append("expand: " + str(ex)[:120])
    natt = pg.eval_on_selector_all("#schemaGraph .ubox-attr", "e=>e.length")
    print(f"DETAIL chemotion: after expand, ubox-attr rows={natt}")
    pg.locator("#schemaGraph").screenshot(path=os.path.join(SHOTS, "schema-expanded.png"))

    # open the files dropdown for the screenshot
    try:
        pg.click(".files-dd summary")
        pg.wait_for_timeout(400)
    except Exception:
        pass
    pg.screenshot(path=os.path.join(SHOTS, "detail-chemotion.png"), full_page=True)

    # --- detail: scholar (header-only -> labelled abstract thumb) ---
    pg.goto(BASE + "/dataset.html?key=scholar", wait_until="load", timeout=60000)
    pg.wait_for_selector(".detail-hero .art img", timeout=30000)
    pg.wait_for_timeout(1500)
    pg.screenshot(path=os.path.join(SHOTS, "detail-scholar.png"))

    b.close()

print(f"\n=== CONSOLE ERRORS ({len(errors)}) ===")
for e in errors[:50]:
    print(" -", e[:300])
print(f"\n=== WARNINGS ({len(warns)}) — first 8 ===")
for w in warns[:8]:
    print(" -", w[:200])
print("\nscreenshots in", SHOTS)
