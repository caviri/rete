import { chromium } from "playwright";
const b = await chromium.launch({ args:["--use-gl=angle","--use-angle=swiftshader","--enable-unsafe-webgpu"] });
const shots = [
  ["subtitles.html", "subtitles-guide", 6000],
  ["anatomy.html", "anatomy-guide", 12000],
  ["lombardi.html", "lombardi-guide", 12000],
  ["webgpu.html", "webgpu-guide", 6000],
  ["plaza/index.html", "plaza-guide", 8000],
];
for (const [page, out, wait] of shots) {
  try {
    const p = await b.newPage({ viewport:{width:1280,height:820}, deviceScaleFactor:1.5 });
    await p.goto(`http://localhost:8080/${page}`, { waitUntil:"domcontentloaded", timeout:40000 });
    await p.waitForTimeout(wait);
    await p.screenshot({ path:`/work/docs/img/${out}.png` });
    const title = await p.title();
    console.log(`${out}: OK (${title.slice(0,40)})`);
    await p.close();
  } catch (e) { console.log(`${out}: FAIL ${String(e).slice(0,80)}`); }
}
await b.close(); process.exit(0);
