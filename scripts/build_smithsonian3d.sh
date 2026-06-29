#!/bin/sh
# Build web/smithsonian3d.rete from the harvested Smithsonian Open Access 3D models.
# Run in the dev container: sh scripts/build_smithsonian3d.sh
set -e
./target/release/rete build data/smithsonian3d/smithsonian3d.nt -o web/smithsonian3d.rete \
  --pyramid-algo types --text-index --card \
  --title "Smithsonian 3D" \
  --description "Interactive 3D models from the Smithsonian Open Access program (CC0, public domain): 2,199 objects, every one streaming a Draco-compressed .glb you can rotate, zoom and inspect right in the browser. Drawn from across the Institution - the National Museum of Natural History leads (skulls, fossils and zoological specimens), with Air and Space, American History, the Portrait Gallery, Cooper Hewitt, African American History and the Freer|Sackler - each model carrying its title, museum unit, catalogue number and a link to the full Smithsonian record. Built keyless from the public smithsonian-open-access S3 bucket (the 3d/ prefix, Voyager scene.svx.json) by scripts/smithsonian3d_to_nt.py; the Medium-quality Draco .glb is taken as the streamable mesh. CC0 1.0 - free to use, share and remix." \
  --license "CC0 1.0 (public domain) - Smithsonian Open Access" \
  --source "https://www.si.edu/openaccess | https://3d.si.edu | s3://smithsonian-open-access/3d" \
  --created 2026-06-29
./target/release/rete info web/smithsonian3d.rete | grep -iE "quad_count|term_count" || true
