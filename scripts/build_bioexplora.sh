#!/bin/sh
# Build web/bioexplora.rete from the harvested graph. Run in the dev container.
set -e
./target/release/rete build data/bioexplora/bioexplora.nt -o web/bioexplora.rete \
  --pyramid-algo types --text-index --card \
  --title "Bioexplora — Museu de Ciencies Naturals de Barcelona" \
  --description "The open natural-history collections of the Museu de Ciencies Naturals de Barcelona (bioexplora.cat), as one queryable graph: 207,163 specimens in Darwin Core across the six MCNB collections — arthropods (MCNB-Art), molluscs (MCNB-Malac), vertebrates (MCNB-Cord), paleontology (MGB), the tissue bank and general zoology — each with full taxonomy (kingdom..species), catalogue number, collector, date, locality, georeference and type status (674+ types), using the real Darwin Core term IRIs. 13,543 specimen IMAGES are linked from the museum IIIF server (iiif.coeli.cat, CORS-open) and render inline; 43,826 records are GEOREFERENCED (GeoSPARQL points, mappable); 667 skull and bone 3D SCANS from the Atles osteologic link out to their Sketchfab viewers; and 173 nature SOUND recordings play inline. Harvested keyless from GBIF (the museum publishes its collections as Darwin Core archives), the Sketchfab account laboratorinatura, and Xeno-canto, by scripts/bioexplora_to_nt.py. Specimens and images are CC BY 4.0 (credit MCNB); the 3D models are CC BY (Sketchfab/MCNB); the audio is CC BY-NC-ND by Eloisa Matheu / Xeno-canto (NOT attributable to the MCNB)." \
  --license "CC BY 4.0 (MCNB specimens & images); 3D models CC BY; audio CC BY-NC-ND (E. Matheu / Xeno-canto, not MCNB)" \
  --source "https://www.bioexplora.cat | GBIF ipt.gbif.es | Sketchfab laboratorinatura | xeno-canto.org" \
  --created 2026-06-30
./target/release/rete info web/bioexplora.rete | grep -iE "quad_count|term_count|pyramid_levels"
ls -la web/bioexplora.rete | awk '{print $5, $9}'
