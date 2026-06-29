#!/bin/sh
# Build web/lineara.rete from the corpus + the INSCRIBE 3D links + the PAITO links.
# Run in the dev container: sh scripts/build_lineara.sh
set -e
B=./target/release/rete
"$B" build \
  data/lineara/lineara.nt \
  data/lineara/lineara-3d.nt \
  data/lineara/lineara-paito.nt \
  -o web/lineara.rete \
  --pyramid-algo types --text-index --card \
  --title "Linear A corpus" \
  --description "The complete surviving corpus of Linear A, the undeciphered script of Minoan Crete (c. 1800-1450 BC): 1721 inscriptions linked through their shared signs and word-sequences, with site, scribe, support, period and transliteration. Text after GORILA (Godart & Olivier) and the tabulation of George Douros; compiled by mwenge (LinearA Explorer). Two scholarly archives are linked in: prop:model3d to the ERC INSCRIBE project 3D scans (67 artifacts, University of Bologna); prop:paito to the PAITO Project (Sapienza, A. Greco) - per-artifact pages for the Haghia Triada sealings and the Phaistos catalogue. LINKS only - inscription images (c) EFA and the 3D/2D+ models (c) INSCRIBE and PAITO (all rights reserved, non-profit scientific use) are NOT included." \
  --license "No explicit license on source repo; scholarly data, attribution required. 3D/2D+ links (c) INSCRIBE (Univ. Bologna) and PAITO (Sapienza) - acknowledge the projects." \
  --source "https://lineara.xyz | github.com/mwenge/lineara.xyz | 3D: inscribercproject.com | PAITO: paitoproject.it" \
  --created 2026-06-29
