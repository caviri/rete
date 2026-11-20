// The card view's decisions (src/view/cards.js), in Node: what a value is, which
// addresses may load, what the host's policy allows, how many cards are drawn.
import assert from "node:assert/strict";
import test from "node:test";

import {
  CARDS_FIRST,
  CARDS_MAX,
  CARDS_STEP,
  FIELD_TYPE_NAMES,
  cardFields,
  cardsShown,
  detect,
  field,
  hostAllows,
  moreLabel,
  safeUrl,
  sourceCovers,
  thumbnail,
} from "../src/view/cards.js";

test("the field types are the playground's, less what needs a service the page cannot reach", () => {
  assert.deepEqual(FIELD_TYPE_NAMES, ["auto", "text", "link", "button", "image", "audio", "video", "model3d", "pdf", "number"]);
});

test("a value is recognised from the value alone", () => {
  const cases = {
    "https://upload.wikimedia.org/wikipedia/commons/a/a9/Example.jpg": "image",
    "https://example.org/pic.PNG?size=large": "image",
    "http://commons.wikimedia.org/wiki/Special:FilePath/Monet.jpg": "image",
    "https://example.org/a/b.svg#x": "image",
    "https://example.org/song.mp3": "audio",
    "https://example.org/clip.webm": "video",
    "https://example.org/model.glb": "model3d",
    "https://example.org/model.gltf?v=2": "model3d",
    "https://example.org/paper.pdf": "pdf",
    "https://example.org/page": "link",
    "http://ex/thing": "text", // a toy namespace: no dotted host
    "Ada Lovelace": "text",
    "see https://example.org/x.jpg": "text", // prose, not an address
    "ftp://example.org/x.jpg": "text",
    "javascript:alert(1)": "text",
    "data:image/png;base64,iVBORw0KGgo=": "image",
    "data:text/html;base64,PHNjcmlwdD4=": "text",
  };
  for (const [value, type] of Object.entries(cases)) assert.equal(detect(value), type, value);
});

test("only http(s) addresses load, data: only for pictures, and nothing with markup in it", () => {
  assert.equal(safeUrl("https://example.org/a b"), null, "a space");
  assert.equal(safeUrl('https://example.org/"onerror=alert(1)'), null);
  assert.equal(safeUrl("https://example.org/<script>"), null);
  assert.equal(safeUrl("javascript:alert(1)"), null);
  assert.equal(safeUrl("JaVaScRiPt:alert(1)"), null);
  assert.equal(safeUrl("vbscript:x"), null);
  assert.equal(safeUrl("file:///etc/passwd"), null);
  assert.equal(safeUrl("//example.org/x"), null);
  assert.equal(safeUrl("/relative.png"), null);
  assert.equal(safeUrl("data:image/png;base64,AAAA"), null, "not unless the field is a picture");
  assert.equal(safeUrl("data:image/png;base64,AAAA", { data: true }), "data:image/png;base64,AAAA");
  assert.equal(safeUrl("data:text/html,<script>alert(1)</script>", { data: true }), null, "never HTML");
  assert.equal(safeUrl("data:image/svg+xml;utf8,<svg/>", { data: true }), "data:image/svg+xml;utf8,<svg/>", "an SVG picture, which an img never runs");
  assert.equal(safeUrl("http://example.org/a.png", { upgrade: true }), "https://example.org/a.png");
  assert.equal(safeUrl("http://example.org/a.png"), "http://example.org/a.png");
  assert.equal(safeUrl("x".repeat(9000)), null);
});

test("a field is what its value is, unless forced; a forced type that does not fit is drawn as text", () => {
  const img = field("photo", "http://example.org/a.jpg");
  assert.equal(img.type, "image");
  assert.equal(img.src, "https://example.org/a.jpg", "upgraded for loading");
  assert.equal(img.link, "http://example.org/a.jpg", "opened as given");
  assert.equal(field("name", "Ada").type, "text");
  assert.equal(field("name", "Ada", "image").type, "text", "no picture of a name");
  assert.equal(field("n", "12x", "number").type, "text");
  assert.equal(field("n", "12.5", "number").type, "number");
  const forced = field("u", "https://example.org/blob?id=7", "video");
  assert.deepEqual([forced.type, forced.src], ["video", "https://example.org/blob?id=7"]);
  assert.equal(field("u", "https://example.org/x.jpg", "text").type, "text", "text stops a picture");
  assert.equal(field("u", "https://example.org/x.jpg", "link").type, "link");
  assert.equal(field("u", "https://example.org/x.jpg", "button").type, "button");
  assert.equal(field("u", "https://example.org/x", "model3d").type, "model3d", "forced 3D loads whatever the extension");
  assert.equal(field("u", "https://example.org/x.jpg", "nonsense").type, "image", "an unknown type is auto");
  const data = field("p", "data:image/gif;base64,R0lGODlhAQABAAAAACw=");
  assert.equal(data.type, "image");
  assert.equal(data.link, null, "a data: address is not opened in a tab");
  assert.equal(thumbnail("https://commons.wikimedia.org/wiki/Special:FilePath/A.jpg", 300), "https://commons.wikimedia.org/wiki/Special:FilePath/A.jpg?width=300");
  assert.equal(thumbnail("https://example.org/a.jpg"), "https://example.org/a.jpg");
});

test("a card skips what the row has no value for, in column order", () => {
  const f = cardFields(["name", "photo", "gone", "n"], { n: "3", photo: "https://example.org/a.jpg", name: "Ada", gone: "" }, { n: "number" });
  assert.deepEqual(f.map((x) => [x.name, x.type]), [["name", "text"], ["photo", "image"], ["n", "number"]]);
});

test("cards are drawn a screenful at a time and never more than the cap", () => {
  assert.equal(cardsShown(10), 10);
  assert.equal(cardsShown(1000), CARDS_FIRST);
  assert.equal(cardsShown(1000, CARDS_FIRST, true), CARDS_FIRST + CARDS_STEP);
  assert.equal(cardsShown(1000, 480, true), CARDS_MAX);
  assert.equal(cardsShown(Infinity, CARDS_MAX, true), CARDS_MAX);
  assert.equal(moreLabel(1000, CARDS_FIRST), `Show ${CARDS_STEP} more (${CARDS_MAX - CARDS_FIRST} hidden)`);
  assert.equal(moreLabel(30, CARDS_FIRST), "Show 6 more (6 hidden)");
  assert.equal(moreLabel(24, 24), "");
  assert.equal(moreLabel(900, CARDS_MAX), "");
});

test("the host's policy: origins, wildcard subdomains, schemes", () => {
  assert.ok(sourceCovers("https://upload.wikimedia.org", "https://upload.wikimedia.org/x.jpg"));
  assert.ok(!sourceCovers("https://upload.wikimedia.org", "https://commons.wikimedia.org/x.jpg"));
  assert.ok(sourceCovers("https://*.wikimedia.org", "https://upload.wikimedia.org/x.jpg"));
  assert.ok(!sourceCovers("https://*.wikimedia.org", "https://wikimedia.org/x.jpg"), "a wildcard is for subdomains");
  assert.ok(!sourceCovers("https://*.wikimedia.org", "https://evilwikimedia.org/x.jpg"));
  assert.ok(!sourceCovers("https://*.wikimedia.org", "https://upload.wikimedia.org.evil.example/x.jpg"));
  assert.ok(sourceCovers("https:", "https://anywhere.example/x"));
  assert.ok(!sourceCovers("https:", "http://anywhere.example/x"));
  assert.ok(sourceCovers("*", "http://anywhere.example/x"));
  assert.ok(sourceCovers("https://a.example:8443", "https://a.example:8443/x"));
  assert.ok(!sourceCovers("https://a.example:8443", "https://a.example/x"));
  assert.ok(!sourceCovers("https://a.example", "http://a.example/x"));
});

test("hostAllows: pictures and media by resourceDomains, 3D by connectDomains, no answer when the host said nothing", () => {
  const csp = { resourceDomains: ["https://*.wikimedia.org"], connectDomains: ["https://data.example.org"] };
  assert.equal(hostAllows(csp, "https://upload.wikimedia.org/a.jpg", "image"), true);
  assert.equal(hostAllows(csp, "https://other.example/a.jpg", "image"), false);
  assert.equal(hostAllows(csp, "https://other.example/a.mp4", "video"), false);
  assert.equal(hostAllows(csp, "https://upload.wikimedia.org/m.glb", "model3d"), false, "a model is fetched, so connect-src");
  assert.equal(hostAllows(csp, "https://data.example.org/m.glb", "model3d"), true);
  assert.equal(hostAllows({}, "https://x.example/a.jpg", "image"), false, "a host that approved nothing");
  assert.equal(hostAllows(undefined, "https://x.example/a.jpg", "image"), null, "unknown: try, and show what happened");
  assert.equal(hostAllows(undefined, "data:image/png;base64,AA", "image"), true);
});
