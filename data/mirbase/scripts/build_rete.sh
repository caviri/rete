#!/usr/bin/env bash
# miRBase 22.1 -> data/mirbase/mirbase.rete
#
# Streams the ontology followed by the data triples straight into the builder,
# so no 300 MB .nt has to sit on disk.
#
#   bash data/mirbase/scripts/build_rete.sh
set -o pipefail
export MSYS_NO_PATHCONV=1
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
WINROOT="$(cd "$ROOT" && { pwd -W 2>/dev/null || pwd; })"
cd "$ROOT"

# make sure the Parquet layer is present and current
for f in db_mirna db_mirna_mature gff3_features fasta_mature; do
  [ -f "data/mirbase/parquet/$f.parquet" ] || {
    echo "!! data/mirbase/parquet/$f.parquet missing — run:" >&2
    echo "   bash data/mirbase/scripts/py.sh tables_to_parquet.py" >&2
    echo "   bash data/mirbase/scripts/py.sh fa_to_parquet.py" >&2
    echo "   bash data/mirbase/scripts/py.sh gff3_to_parquet.py" >&2
    exit 1
  }
done

bash data/mirbase/scripts/py.sh make_ontology.py >&2

echo "=== mirbase build start ===" >&2
{ cat data/mirbase/mirbase-ontology.nt
  bash data/mirbase/scripts/py.sh parquet_to_nt.py
} | docker run -i --rm -v "$WINROOT:/work" -w /work rete-dev:latest \
      /work/target/release/rete build - --format nt \
      -o /work/data/mirbase/mirbase.rete --card \
      --title "miRBase" \
      --license "CC0-1.0" \
      --source "https://www.mirbase.org/" \
      --description "miRBase release 22.1 — the reference catalogue of published microRNA sequences — as one range-queryable .rete. 38,589 hairpin stem-loops and 48,885 mature miRNAs across 271 species, with mature-product offsets, miRNA families (MIPF), high-confidence flags, host-transcript context, withdrawn accessions and 47,958 literature citations to PubMed. Genome coordinates come from both the 31 curated assembly-stamped GFF3 files (GRCh38, GRCm38, …) and the 153-species mirna_chromosome_build table, expressed as FALDO regions. Species are NCBITaxon IRIs and stem-loops/matures carry RNAcentral, Rfam, EntrezGene, HGNC and MGI cross-references, so the graph joins the wider biological linked-data world by IRI. Modelled with the rete mb: vocabulary (Sequence Ontology / FALDO / NCBITaxon / dcterms / SKOS / FaBiO)."

rc=$?
echo "=== build exit code: $rc ===" >&2
ls -la data/mirbase/mirbase.rete 2>/dev/null
exit $rc
