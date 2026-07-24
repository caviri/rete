#!/usr/bin/env bash
# Reproducible harvest of the ETH Research Collection metadata via OAI-PMH.
#
# Source: https://www.research-collection.ethz.ch/  (DSpace 7.6)
# OAI base: https://www.research-collection.ethz.ch/server/oai/<context>
# Docs:  https://unlimited.ethz.ch/spaces/RC/pages/194119646/OAI-PMH+interface
#
# We harvest the `all_items` context in the native `oai_ethz` metadata format —
# the richest crosswalk (authors+ORCID, affiliation/Leitzahl tree, DOI/arXiv/WoS/
# Scopus/ISSN/ISBN IDs, grants, DDC/JEL, license, abstract). This is the COMPLETE
# dump: 306,835 records on 2026-07-23. oai_ethz is offered ONLY on all_items.
#
# Everything runs in Docker (python:3.12-slim, stdlib only). data/ is gitignored;
# this script + harvest_oai.py reconstruct raw/ from scratch.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
REL="data/ethz-research-collection"

run() {  # run a harvester invocation in Docker, cwd = repo root
  MSYS_NO_PATHCONV=1 docker run --rm -v "${REPO_ROOT}:/w" -w //w python:3.12-slim \
    python "${REL}/scripts/harvest_oai.py" "$@"
}

# 1. Full metadata — native oai_ethz crosswalk, ALL reviewed items (the dump).
#    Nicely structured (author→name+orcid, grant sub-objects) but a *curated*
#    crosswalk: drops a few fields (relation.*, dspace.entity.type, rosetta admin).
run --context all_items --prefix oai_ethz --out "${REL}/raw/oai_ethz"

# 2. COMPLETE DUMP — xoai is the raw DSpace registry and, on this instance, the
#    full enrichment in one format. Per record it carries, beyond every metadata
#    field: the entity RELATIONSHIP graph (<relation> → isAuthorOfPublication /
#    isJournalOfPublication / isPub*{New,Prev}Version / isPubCites|ReferencedBy /
#    isSupervisorOf… with the linked entity UUID + virtual:: authority), and the
#    full FILE MANIFEST (<bundles>/<bundle>/<bitstreams>/<bitstream>: name, format
#    (MIME), size (bytes), url (download link), checksum + checksumAlgorithm=MD5).
#    i.e. everything the REST API /items/{uuid}?embed=bundles,relationships gives,
#    WITHOUT downloading any bitstream bytes. Authoritative source for the .rete.
run --context all_items --prefix xoai --out "${REL}/raw/xoai"

# 3. Sets vocabulary — the community/collection tree (tiny), for later modelling.
run --context all_items --verb ListSets --out "${REL}/raw/sets"

# --- optional enrichment (not run by default) --------------------------------
# DataCite kernel-4 XML (structured relatedIdentifiers → citation/version edges)
# is offered only on the `request`/`openaire` contexts, i.e. the ~114k full-text
# subset, NOT on all_items. Uncomment to also grab it:
# run --context request --prefix oai_datacite --out "${REL}/raw/oai_datacite"

echo "harvest complete → ${REL}/raw/" >&2
