// The gate's .rete fixtures must be the ones tests/gate/fixtures.sh produced
// from tests/gate/fixtures/manifest.json — no substitutions, no leftovers from
// an older recipe.
//
// gate.sh runs the producer before it runs the gate, so on that path this is a
// tautology. It is not on the others: `docker compose run --rm gate`,
// `gate-firefox` and the catalog sweeps all invoke `node run.mjs` directly, and
// CI runs it after its own build step. Those paths get whatever happens to be
// sitting in tests/gate/.cache — which is exactly how a downloaded
// worldcup2026.rete (16,184 triples, WITH a card) came to stand in for the
// 7-triple cardless build and reddened check_card_modal instead of itself.
//
// This check cannot re-derive the fixtures: the Playwright image has no Rust.
// It verifies the STAMP the producer wrote — same recipe, same sources, same
// bytes — which is enough to name the fixture and the command that repairs it.
// The stamp is written per build and not committed: a build record carries a
// timestamp and measured milliseconds, so two builds are legitimately different
// files and a checked-in hash would be a lie.
import fs from "node:fs";
import crypto from "node:crypto";
import { expect } from "./_expect.mjs";

const ROOT = process.env.RETE_ROOT || "/work";
const SRC = `${ROOT}/tests/gate/fixtures`;
const CACHE = `${ROOT}/tests/gate/.cache`;
const RUN = "run `bash tests/gate/fixtures.sh --force`";

const t = expect("check_fixture_provenance");
const sha256 = (buf) => crypto.createHash("sha256").update(buf).digest("hex");

const manifestBytes = fs.readFileSync(`${SRC}/manifest.json`);
const manifest = JSON.parse(manifestBytes.toString("utf8"));
const stampPath = `${CACHE}/fixtures.stamp.json`;

if (!fs.existsSync(stampPath)) {
  t.fail(
    "stamp",
    `tests/gate/.cache/fixtures.stamp.json is missing — these fixtures were never produced by ` +
    `the recipe, so nothing knows what they are supposed to be; ${RUN}`,
  );
  t.finish({ fixtures: 0 });
} else {
  const stamp = JSON.parse(fs.readFileSync(stampPath, "utf8"));

  // The recipe itself may have moved on since the fixtures were built.
  t.equal(
    "recipe",
    stamp.manifestSha256 === sha256(manifestBytes) ? "current" : "stale",
    "current",
    `the fixtures in tests/gate/.cache were built from an older tests/gate/fixtures/manifest.json; ${RUN}`,
  );
  for (const [name, hash] of Object.entries(stamp.sourceSha256 || {})) {
    t.equal(
      `source:${name}`,
      sha256(fs.readFileSync(`${SRC}/${name}`)) === hash ? "current" : "edited",
      "current",
      `tests/gate/fixtures/${name} changed after the fixtures were built; ${RUN}`,
    );
  }

  // …and the files themselves may have been replaced by a download or a hand copy.
  for (const fx of manifest.fixtures) {
    const path = `${CACHE}/${fx.out}`;
    const recorded = (stamp.fixtures || {})[fx.out];
    if (!recorded) {
      t.fail(`stamped:${fx.out}`, `${fx.out} is not in the producer's stamp; ${RUN}`);
      continue;
    }
    if (!fs.existsSync(path)) {
      t.fail(
        `present:${fx.out}`,
        `tests/gate/.cache/${fx.out} is gone (needed by: ${(fx.usedBy || []).join("; ")}); ${RUN}`,
      );
      continue;
    }
    const bytes = fs.readFileSync(path);
    t.equal(
      `bytes:${fx.out}`,
      sha256(bytes) === recorded.sha256 ? "as-built" : "replaced",
      "as-built",
      `tests/gate/.cache/${fx.out} is ${bytes.length} B against the ${recorded.bytes} B the ` +
      `producer wrote — something replaced the built fixture ` +
      `(needed by: ${(fx.usedBy || []).join("; ")}); ${RUN}`,
    );
    // The property every consuming check silently depends on, restated where
    // the gate can see it: card-carrying or not.
    t.equal(
      `card:${fx.out}`,
      recorded.card ? "carded" : "cardless",
      fx.assert.card ? "carded" : "cardless",
      `${fx.out} was built ${recorded.card ? "WITH" : "WITHOUT"} a Dataset Card and the recipe ` +
      `says the opposite; ${RUN}`,
    );
  }

  const builders = [...new Set(
    Object.values(stamp.fixtures || {}).map((f) => f.builder).filter(Boolean),
  )];
  t.finish({ fixtures: manifest.fixtures.length, builders });
}
