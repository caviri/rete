#!/usr/bin/env node
/* Link Linear A inscriptions to their INSCRIBE 3D models.
 *
 * The ERC INSCRIBE project (University of Bologna, Prof. Silvia Ferrara) publishes
 * high-resolution 3D scans (3DHOP web viewers) of a curated set of Linear A
 * artifacts at inscribercproject.com. We add a `prop:model3d` LINK (a reference /
 * citation — NOT the model bytes) from each matching inscription to its public
 * viewer page. The 3D models themselves are © INSCRIBE / Università di Bologna,
 * all rights reserved (non-profit scientific use); linking + attribution is exactly
 * what their data policy asks for.
 *
 * A physical tablet ("HT 114") is often several inscription records in the corpus
 * (HT114a, HT114b — its inscribed faces), so each face is linked to the one scan.
 *
 * Usage: node scripts/lineara_inscribe.js data/lineara/codes.txt > data/lineara/lineara-3d.nt
 *   (codes.txt = one inscription code per line, dumped from the .rete)
 */
const fs = require("fs");

const BASE = "https://lineara.xyz/";
const MODEL3D = BASE + "prop/model3d";
const SITE = "https://www.inscribercproject.com/";

// The complete INSCRIBE Linear A 3D catalogue (inscribercproject.com/Linear_A.php),
// as { code, file } — code in GORILA notation, file the exact viewer page.
const INSCRIBE = [
  // --- Museo delle Civiltà, Rome ---
  ["HT 29",  "Clay_tablet_HT_29.php"],
  ["HT 114", "Clay_tablet_HT_114.php"],
  ["HT 118", "Clay_tablet_HT_118.php"],
  ["HT Wa 1014", "Clay_nodule_HT_Wa_1014.php"],
  ["HT Wa 1561", "Clay_nodule_HT_Wa_1561.php"],
  ["HT Wa 1779", "Clay_nodule_HT_Wa_1779.php"],
  ["HT Zb 160",  "Clay_vessel_HT_Zb_160.php"],
  // --- Archaeological Museum of Chania (Khania) ---
  ...[5,6,7,8,9,10,11,12,14,15,18,19,20,21,22,24,25,53,60,73,86,88,96,104]
      .map(n => [`KH ${n}`, `Clay_tablet_KH_${n}.php`]),
  ["KH Wa 1001", "Clay_nodule_KH_Wa_1001.php"],
  // dual-classified Wa/Wd nodules; the corpus codes them under Wa (KHWa1003/1004)
  ["KH Wa 1003", "Clay_nodule_KH_Wa_Wd_1003.php"],
  ["KH Wa 1004", "Clay_nodule_KH_Wa_Wd_1004.php"],
  ["KH Wa 1018", "Clay_nodule_KH_Wa_1018.php"],
  ["KH Wa 1019", "Clay_nodule_KH_Wa_1019.php"],
  ["KH Wc 2002", "Clay_roundel_KH_Wc_2002.php"],
  ["KH Wc 2014", "Clay_roundel_KH_Wc_2014.php"],
  ["KH Wc 2029", "Clay_roundel_KH_Wc_2029.php"],
  // --- Archaeological Museum of Heraklion (Malia / Phaistos / Knossos) ---
  ["MA 4", "Clay_tablet_MA_4.php"],
  ["MA 6", "Clay_tablet_MA_6.php"],
  ["MA 9", "Clay_tablet_MA_9.php"],
  ...[1,2,3,6,7,8,10,11,16,17,18,19,24,25,27,28,30]
      .map(n => [`PH ${n}`, `Clay_tablet_PH_${n}.php`]),
  ["KN 1", "Clay_tablet_KN_1.php"],
  ["MA 1", "Clay_bar_MA_1.php"],
  ["MA 2", "Clay_bar_MA_2.php"],
  ...[13,14,15,22,26].map(n => [`PH ${n}`, `Clay_bar_PH_${n}.php`]),
];

const codes = fs.readFileSync(process.argv[2] || "data/lineara/codes.txt", "utf8")
  .split(/\r?\n/).map(s => s.trim()).filter(Boolean);

// A data code C belongs to base B iff C === B, or C = B + a face suffix that starts
// with a LETTER (so "HT114"+"a"/"b" match but "HT11"+"4" and "HT114"+"0" do not).
function facesOf(base){
  return codes.filter(c => {
    if (!c.startsWith(base)) return false;
    const rest = c.slice(base.length);
    return rest === "" || /^[a-zA-Z]/.test(rest);
  });
}

const out = [];
const matched = [], unmatched = [];
for (const [code, file] of INSCRIBE) {
  const base = code.replace(/\s+/g, "").replace(/\//g, "");
  const faces = facesOf(base);
  if (!faces.length) { unmatched.push(code); continue; }
  matched.push(`${code} → ${faces.length}`);
  const url = SITE + file;
  for (const f of faces) {
    out.push(`<${BASE}inscription/${encodeURIComponent(f)}> <${MODEL3D}> <${url}> .`);
  }
}

process.stderr.write(
  `inscribe: ${matched.length}/${INSCRIBE.length} artifacts matched, ${out.length} model3d links\n` +
  (unmatched.length ? `  unmatched: ${unmatched.join(", ")}\n` : "  (all matched)\n"));
process.stdout.write(out.join("\n") + "\n");
