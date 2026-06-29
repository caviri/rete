#!/usr/bin/env node
/* Link Linear A inscriptions to the PAITO Project (paitoproject.it).
 *
 * The PA-I-TO Project (Prof. Alessandro Greco, Sapienza Università di Roma, with
 * Georgia Flouda / Heraklion Museum and Erika Notti / IULM) is a digital archive of
 * the Linear A documents of Phaistos AND Haghia Triada — tablets, clay sealings
 * (cretulae) and vases, with 2D+/3D imaging. We add `prop:paito` LINKS (references,
 * NOT the media): the 2D+/3D models are © PAITO, all rights reserved, non-profit
 * scientific use — linking + attribution is what their terms ask for.
 *
 * Two tiers, by what PAITO has actually published:
 *  - Haghia Triada sealings (HT Wa NNNN): real per-artifact pages exist (in the
 *    site's sitemap) → DEEP link each face to /ht-wa-NNNN/.
 *  - Phaistos (PH …): the per-artifact tablet pages are still "Coming soon", so we
 *    link each PH document to its PAITO Phaistos sub-CATALOGUE (tablets / clay
 *    sealings / vases) rather than fabricate a dead per-artifact URL.
 *
 * Usage: node scripts/lineara_paito.js data/lineara/codes.txt > data/lineara/lineara-paito.nt
 */
const fs = require("fs");

const BASE = "https://lineara.xyz/";
const PAITO = BASE + "prop/paito";
const SITE = "https://www.paitoproject.it/";

// Haghia Triada sealings with a published per-artifact page (canonical slugs from
// paitoproject.it/page-sitemap.xml). All map to HTWaNNNN in the corpus.
const HT_WA = [1014, 1108, 1110, 1150, 1176, 1283, 1294, 1301, 1407, 1408, 1472,
  1512, 1542, 1547, 1559, 1560, 1561, 1593, 1623, 1744, 1759, 1779, 1830];

// Phaistos sub-catalogues (per-artifact pages forthcoming).
const PH_TABLETS = SITE + "en/tablets-2/";
const PH_SEALINGS = SITE + "en/clay-sealings/";
const PH_VASES = SITE + "en/vases/";

const codes = fs.readFileSync(process.argv[2] || "data/lineara/codes.txt", "utf8")
  .split(/\r?\n/).map(s => s.trim()).filter(Boolean);

// match BASE itself or BASE + a face suffix that starts with a letter
function facesOf(base){
  return codes.filter(c => c.startsWith(base) &&
    (c.length === base.length || /^[a-zA-Z]/.test(c.slice(base.length))));
}
const link = (code, url) => `<${BASE}inscription/${encodeURIComponent(code)}> <${PAITO}> <${url}> .`;

const out = [];
let deepArtifacts = 0, deepLinks = 0;
for (const n of HT_WA) {
  const faces = facesOf(`HTWa${n}`);
  if (!faces.length) continue;
  deepArtifacts++;
  const url = `${SITE}ht-wa-${n}/`;
  for (const f of faces) { out.push(link(f, url)); deepLinks++; }
}

// Every Phaistos document → its PAITO sub-catalogue (by GORILA class in the code).
let phLinks = 0;
for (const c of codes) {
  if (!/^PH/.test(c)) continue;
  const url = /^PH\(?\??\)?W[a-z]/.test(c) ? PH_SEALINGS   // PH Wa… sealings
            : /^PH\(?\??\)?Z/.test(c)       ? PH_VASES      // PH Zb… vases
            :                                 PH_TABLETS;   // PH tablets/bars
  out.push(link(c, url)); phLinks++;
}

process.stderr.write(
  `paito: HT Wa deep links = ${deepLinks} (${deepArtifacts} artifacts); ` +
  `Phaistos catalogue links = ${phLinks}; total = ${out.length}\n`);
process.stdout.write(out.join("\n") + "\n");
