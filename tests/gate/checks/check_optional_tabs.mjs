// Non-downloading initialization smoke for Ask AI + Semantic/RAG. WebGPU is
// unavailable by design; model and embedding work is replaced by deterministic
// local stubs so controls and failure copy can be release-gated without inference.
import { launchBrowser } from "./_browser.mjs";

const main = async () => {
  const browser = await launchBrowser();
  const page = await browser.newPage();
  const errs = [];
  const externalModelRequests = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 240)));
  page.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 200)); });
  page.on("request", (r) => { if (/huggingface|transformers|cdn\.jsdelivr|static\.hf\.space/i.test(r.url())) externalModelRequests.push(r.url()); });
  await page.addInitScript(() => {
    try { Object.defineProperty(navigator, "gpu", { configurable: true, value: undefined }); } catch (_) {}
    Object.defineProperty(window, "RETE_PLAYGROUND_CATALOG", {
      configurable: true,
      set(value) {
        value.rag = value.rag || {};
        value.rag.scholar = {
          emb: "/__gate__/rag.f32", index: "/__gate__/rag.json", model: "gate/stub-model",
          queryPrefix: "query: ", dim: 2, count: 2,
        };
        Object.defineProperty(window, "RETE_PLAYGROUND_CATALOG", { configurable: true, writable: true, value });
      },
    });
    window.Worker = class GateWorker {
      postMessage(message) {
        queueMicrotask(() => {
          if (message.type === "load") this.onmessage?.({ data: { type: "ready" } });
          else if (message.type === "embed") this.onmessage?.({ data: { type: "vec", vec: [1, 0] } });
          else if (message.type === "generate") {
            this.onmessage?.({ data: { type: "token", text: "Stub grounded answer." } });
            this.onmessage?.({ data: { type: "done" } });
          }
        });
      }
      terminate() {}
    };
  });
  const f32 = Buffer.alloc(16);
  f32.writeFloatLE(1, 0); f32.writeFloatLE(0, 4); f32.writeFloatLE(0, 8); f32.writeFloatLE(1, 12);
  await page.route("**/__gate__/rag.f32", (route) => route.fulfill({ status: 200, contentType: "application/octet-stream", body: f32 }));
  await page.route("**/__gate__/rag.json", (route) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify([
    { iri: "http://example.test/alice", title: "Alice" },
    { iri: "http://example.test/bob", title: "Bob" },
  ]) }));

  const PORT = process.env.PGPORT || "8090";
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=scholar&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("askAiBtn"), { timeout: 60000 });

  await page.click("#askAiBtn");
  const ask = await page.evaluate(() => ({
    visible: !!document.querySelector(".ai-modal:not(.hidden)"),
    hasDialog: !!document.querySelector(".ai-modal [role=dialog]"),
    copy: (document.querySelector(".ai-modal .ai-warn") || {}).textContent || "",
  }));
  await page.click(".ai-modal-close");

  const semanticButton = 'button[data-mode="semantic"]';
  const semanticVisible = await page.isVisible(semanticButton);
  await page.click(semanticButton);
  await page.waitForFunction(() => /Ready/.test((document.getElementById("semanticOut") || {}).textContent || ""), { timeout: 15000 });
  await page.fill("#semanticQ", "Alice");
  await page.click("#semanticGo");
  await page.waitForFunction(() => /Alice/.test((document.getElementById("semanticOut") || {}).textContent || "") && !document.getElementById("semanticAnswerWrap")?.classList.contains("hidden"), { timeout: 10000 });
  await page.click("#semanticAnswerBtn");
  await page.waitForFunction(() => /Stub grounded answer/.test((document.getElementById("semanticAnswer") || {}).textContent || ""), { timeout: 10000 });
  const semantic = await page.evaluate(() => ({
    barVisible: !document.getElementById("semanticBar")?.classList.contains("hidden"),
    hasSearch: !!document.getElementById("semanticQ") && !!document.getElementById("semanticGo"),
    result: (document.getElementById("semanticOut") || {}).textContent || "",
    ragControl: !document.getElementById("semanticAnswerWrap")?.classList.contains("hidden") && !!document.getElementById("semanticAnswerBtn"),
    answer: (document.getElementById("semanticAnswer") || {}).textContent || "",
  }));
  const pass = ask.visible && ask.hasDialog && /needs\s+WebGPU/i.test(ask.copy) && semanticVisible &&
    semantic.barVisible && semantic.hasSearch && /Alice/.test(semantic.result) && semantic.ragControl &&
    /Stub grounded answer/.test(semantic.answer) && externalModelRequests.length === 0 && errs.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL", ask, semanticVisible, semantic,
    externalModelRequests: externalModelRequests.slice(0, 3), errs: errs.slice(0, 4),
  }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
