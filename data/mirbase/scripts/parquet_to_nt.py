#!/usr/bin/env python3
"""miRBase Parquet -> N-Triples on stdout (streamed into `rete build -`).

Joins the relational dump, the FASTA sequences and the GFF3 genome coordinates
into one graph:

    mb:StemLoop   (MI…)   --hasMatureProduct-->  mb:MatureMiRNA (MIMAT…)
         |  organism -> NCBITaxon IRI
         |  family   -> MIPF…
         |  location -> faldo:Region -> assembly (GRCh38, …)
         |  dcterms:isReferencedBy -> pubmed.ncbi.nlm.nih.gov IRI
         `  rdfs:seeAlso -> RNAcentral / Rfam / EntrezGene / HGNC / MGI …

Genome coordinates come from BOTH sources, which cover different things:
  * the 31 curated GFF3 files — assembly-stamped, for stem-loops AND matures;
  * mirna_chromosome_build    — 153 species, stem-loops only, no assembly stamp.
Regions from each are typed with the source so a query can tell them apart.

    bash data/mirbase/scripts/py.sh parquet_to_nt.py > mirbase.nt
    # or stream it straight into the builder — see build_rete.sh
"""
from __future__ import annotations

import sys
from pathlib import Path
from urllib.parse import quote

import pyarrow.parquet as pq

PARQ = Path(__file__).resolve().parent.parent / "parquet"

MB = "https://w3id.org/rete/mirbase#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
XSD = "http://www.w3.org/2001/XMLSchema#"
DCT = "http://purl.org/dc/terms/"
SKOS = "http://www.w3.org/2004/02/skos/core#"
FALDO = "http://biohackathon.org/resource/faldo#"
OBO = "http://purl.obolibrary.org/obo/"
FABIO = "http://purl.org/spar/fabio/"

HAIRPIN = "https://www.mirbase.org/hairpin/"
MATURE = "https://www.mirbase.org/mature/"
FAMILY = "https://www.mirbase.org/family/"
SPECIES = "https://www.mirbase.org/species/"
ASSEMBLY = "https://w3id.org/rete/mirbase/assembly/"
REFSEQ = "https://w3id.org/rete/mirbase/reference/"
PUBMED = "https://pubmed.ncbi.nlm.nih.gov/"
NCBITAXON = OBO + "NCBITaxon_"

out = sys.stdout

# Rows that are corrupt in miRBase's OWN mysql export: their tab-separated
# fields are shifted, so `auto_species` holds a value from another column and
# points at the wrong organism. miRBase excludes these from hairpin.fa,
# miRNA.dat and mirna_pre_mature; we keep the entry (it is real) but refuse to
# emit the demonstrably wrong species link, and flag it. See README "Gotchas".
#   MI0023465 bma-mir-5863-2 — auto_species=1 (aqu) but the entry is Brugia malayi
MALFORMED_SOURCE_ROWS = {"MI0023465"}


def esc(s: str) -> str:
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


def t(s: str, p: str, o: str) -> None:
    out.write(f"<{s}> <{p}> {o} .\n")


def lit(v: str) -> str:
    return f'"{esc(v)}"'


def num(v, xtype: str = "integer") -> str:
    return f'"{v}"^^<{XSD}{xtype}>'


def boolean(v: bool) -> str:
    return f'"{str(v).lower()}"^^<{XSD}boolean>'


def load(name: str) -> list[dict]:
    p = PARQ / f"{name}.parquet"
    if not p.exists():
        print(f"!! missing {p}", file=sys.stderr)
        return []
    return pq.read_table(p).to_pylist()


def main() -> None:
    # ---------------------------------------------------------------- species
    species = load("db_mirna_species")
    sp_iri: dict[int, str] = {}       # auto_id -> IRI
    sp_code: dict[int, str] = {}      # auto_id -> 'hsa'
    for s in species:
        code = s["organism"] or ""
        taxon = s["taxon_id"]
        # prefer a real NCBITaxon IRI so the graph joins other biological data
        iri = f"{NCBITAXON}{taxon}" if taxon else f"{SPECIES}{code}"
        sp_iri[s["auto_id"]] = iri
        sp_code[s["auto_id"]] = code

        t(iri, RDF + "type", f"<{MB}Species>")
        if s["name"]:
            t(iri, RDFS + "label", lit(s["name"]))
            t(iri, SKOS + "prefLabel", lit(s["name"]))
        if code:
            t(iri, MB + "organismCode", lit(code))
        if taxon:
            t(iri, MB + "taxonId", num(taxon))
        if s["taxonomy"]:
            t(iri, MB + "taxonomyPath", lit(s["taxonomy"]))
        if s["genome_assembly"]:
            a_iri = f"{ASSEMBLY}{s['genome_assembly'].replace(' ', '_')}"
            t(a_iri, RDF + "type", f"<{MB}GenomeAssembly>")
            t(a_iri, RDFS + "label", lit(s["genome_assembly"]))
            if s["genome_accession"]:
                t(a_iri, MB + "assemblyAccession", lit(s["genome_accession"]))
            t(iri, MB + "assembly", f"<{a_iri}>")

    # ------------------------------------------------------------- stem-loops
    mirna = load("db_mirna")
    acc_of: dict[int, str] = {}       # auto_mirna -> MI accession
    label_of: dict[str, str] = {}     # accession -> the entity's rdfs:label
    for m in mirna:
        acc = m["mirna_acc"]
        acc_of[m["auto_mirna"]] = acc
        iri = HAIRPIN + acc
        t(iri, RDF + "type", f"<{MB}StemLoop>")
        t(iri, DCT + "identifier", lit(acc))
        if m["mirna_id"]:
            label_of[acc] = m["mirna_id"]
            t(iri, RDFS + "label", lit(m["mirna_id"]))
            t(iri, SKOS + "prefLabel", lit(m["mirna_id"]))
        if m["description"]:
            t(iri, DCT + "description", lit(m["description"].strip()))
        # the dump has stray leading/trailing spaces in a few sequence cells
        seq = (m["sequence"] or "").strip()
        if seq:
            t(iri, MB + "sequence", lit(seq))
            t(iri, MB + "sequenceLength", num(len(seq)))
        if m["comment"]:
            t(iri, RDFS + "comment", lit(m["comment"].strip()))
        if m["previous_mirna_id"]:
            for prev in str(m["previous_mirna_id"]).split(";"):
                if prev.strip():
                    t(iri, MB + "previousId", lit(prev.strip()))
                    t(iri, SKOS + "altLabel", lit(prev.strip()))
        if acc in MALFORMED_SOURCE_ROWS:
            t(iri, MB + "sourceRowMalformed", boolean(True))
        else:
            sp = sp_iri.get(m["auto_species"])
            if sp:
                t(iri, MB + "organism", f"<{sp}>")
        t(iri, MB + "deadFlag", boolean(bool(m["dead_flag"])))

    # ---------------------------------------------------------------- matures
    # For 8 entries the relational `mature_name` is stale relative to the name
    # miRBase actually publishes in mature.fa (e.g. MIMAT0001107 is
    # 'gga-miR-222' in the table but 'gga-miR-222a' in the FASTA). The published
    # distribution wins for the label; the table's name is kept as an alt-label.
    fasta_mature_rows = load("fasta_mature")
    fasta_name = {r["accession"]: r["name"] for r in fasta_mature_rows}

    mature = load("db_mirna_mature")
    mat_acc: dict[int, str] = {}      # auto_mature -> MIMAT accession
    for m in mature:
        acc = m["mature_acc"]
        mat_acc[m["auto_mature"]] = acc
        iri = MATURE + acc
        t(iri, RDF + "type", f"<{MB}MatureMiRNA>")
        t(iri, DCT + "identifier", lit(acc))
        label = fasta_name.get(acc) or m["mature_name"]
        if label:
            label_of[acc] = label
            t(iri, RDFS + "label", lit(label))
            t(iri, SKOS + "prefLabel", lit(label))
        if m["mature_name"] and m["mature_name"] != label:
            t(iri, SKOS + "altLabel", lit(m["mature_name"]))
        if m["evidence"]:
            t(iri, MB + "evidence", lit(m["evidence"]))
        if m["experiment"]:
            t(iri, MB + "experiment", lit(m["experiment"]))
        if m["similarity"]:
            t(iri, MB + "similarity", lit(m["similarity"]))
        if m["previous_mature_id"]:
            for prev in str(m["previous_mature_id"]).split(";"):
                if prev.strip():
                    t(iri, MB + "previousId", lit(prev.strip()))
                    t(iri, SKOS + "altLabel", lit(prev.strip()))
        t(iri, MB + "deadFlag", boolean(bool(m["dead_flag"])))

    # mature sequences (and their descriptions) live in the FASTA, keyed by
    # accession — the relational dump carries neither
    for r in fasta_mature_rows:
        iri = MATURE + r["accession"]
        t(iri, MB + "sequence", lit(r["sequence"]))
        t(iri, MB + "sequenceLength", num(r["seq_length"]))
        if r["description"]:
            t(iri, DCT + "description", lit(r["description"]))
    high_conf_mature = {r["accession"] for r in load("fasta_mature_high_conf")}
    for acc in high_conf_mature:
        t(MATURE + acc, MB + "highConfidence", boolean(True))
    high_conf_hairpin = {r["accession"] for r in load("fasta_hairpin_high_conf")}
    for acc in high_conf_hairpin:
        t(HAIRPIN + acc, MB + "highConfidence", boolean(True))

    # ------------------------------------------------- stem-loop -> mature
    # The offset of a mature is a property of the PAIRING, not of the mature:
    # the same mature can sit on several stem-loops at different offsets (e.g.
    # MIMAT0007005 is at 96 on MI0006786 but at 39 on MI0024086). So the
    # offsets hang off an mb:Placement node. They are ALSO put directly on the
    # mature when it has exactly one parent, where that is unambiguous.
    pre_mature = load("db_mirna_pre_mature")
    parent_count: dict[int, int] = {}
    for pm in pre_mature:
        parent_count[pm["auto_mature"]] = parent_count.get(pm["auto_mature"], 0) + 1

    for pm in pre_mature:
        h = acc_of.get(pm["auto_mirna"])
        m = mat_acc.get(pm["auto_mature"])
        if not (h and m):
            continue
        t(HAIRPIN + h, MB + "hasMatureProduct", f"<{MATURE + m}>")
        t(MATURE + m, MB + "derivesFrom", f"<{HAIRPIN + h}>")

        placement = f"{HAIRPIN}{h}/placement/{m}"
        t(HAIRPIN + h, MB + "maturePlacement", f"<{placement}>")
        t(placement, RDF + "type", f"<{MB}Placement>")
        t(placement, MB + "mature", f"<{MATURE + m}>")
        t(placement, MB + "stemLoop", f"<{HAIRPIN + h}>")
        if pm["mature_from"]:
            t(placement, MB + "matureFrom", num(pm["mature_from"]))
        if pm["mature_to"]:
            t(placement, MB + "matureTo", num(pm["mature_to"]))
        if parent_count.get(pm["auto_mature"], 0) == 1:
            if pm["mature_from"]:
                t(MATURE + m, MB + "matureFrom", num(pm["mature_from"]))
            if pm["mature_to"]:
                t(MATURE + m, MB + "matureTo", num(pm["mature_to"]))

    # --------------------------------------------------------------- families
    fam_acc: dict[int, str] = {}
    for f in load("db_mirna_prefam"):
        fam_acc[f["auto_prefam"]] = f["prefam_acc"]
        iri = FAMILY + f["prefam_acc"]
        t(iri, RDF + "type", f"<{MB}Family>")
        t(iri, DCT + "identifier", lit(f["prefam_acc"]))
        if f["prefam_id"]:
            t(iri, RDFS + "label", lit(f["prefam_id"]))
            t(iri, SKOS + "prefLabel", lit(f["prefam_id"]))
        if f["description"]:
            t(iri, DCT + "description", lit(f["description"]))
    for link in load("db_mirna_2_prefam"):
        h = acc_of.get(link["auto_mirna"])
        fa = fam_acc.get(link["auto_prefam"])
        if h and fa:
            t(HAIRPIN + h, MB + "family", f"<{FAMILY + fa}>")

    # ------------------------------------------------------------- literature
    lit_rows = {r["auto_lit"]: r for r in load("db_literature_references")}
    for r in lit_rows.values():
        pmid = r["medline"]
        iri = f"{PUBMED}{pmid}" if pmid else f"{MB}ref/{r['auto_lit']}"
        t(iri, RDF + "type", f"<{FABIO}JournalArticle>")
        if r["title"]:
            t(iri, DCT + "title", lit(r["title"].strip('"')))
        if r["author"]:
            t(iri, DCT + "creator", lit(r["author"]))
        if r["journal"]:
            t(iri, DCT + "bibliographicCitation", lit(r["journal"]))
        if pmid:
            t(iri, DCT + "identifier", lit(f"pmid:{pmid}"))
    for mlr in load("db_mirna_literature_references"):
        h = acc_of.get(mlr["auto_mirna"])
        r = lit_rows.get(mlr["auto_lit"])
        if not (h and r):
            continue
        pmid = r["medline"]
        ref = f"{PUBMED}{pmid}" if pmid else f"{MB}ref/{r['auto_lit']}"
        t(HAIRPIN + h, DCT + "isReferencedBy", f"<{ref}>")

    # ---------------------------------------------------------- cross-refs
    hairpin_db = {r["auto_db"]: r for r in load("db_mirna_database_url")}
    mature_db = {r["auto_db"]: r for r in load("db_mature_database_url")}

    def xref(template: str, value: str) -> str | None:
        if not template or "<?>" not in template:
            return None
        return template.replace("<?>", value)

    for l in load("db_mirna_database_links"):
        h = acc_of.get(l["auto_mirna"])
        db = hairpin_db.get(l["auto_db"])
        if not (h and db and l["link"]):
            continue
        url = xref(db["url"], l["link"].strip())
        if url:
            t(HAIRPIN + h, RDFS + "seeAlso", f"<{url}>")
    for l in load("db_mature_database_links"):
        m = mat_acc.get(l["auto_mature"])
        db = mature_db.get(l["auto_db"])
        if not (m and db and l["link"]):
            continue
        # miRTarBase's template also needs a species code we do not carry here
        url = xref(db["url"], l["link"].strip())
        if url and "<SPECIES>" not in url:
            t(MATURE + m, RDFS + "seeAlso", f"<{url}>")

    # ------------------------------------------------------------- confidence
    for c in load("db_confidence_score"):
        h = acc_of.get(c["auto_mirna"])
        if h:
            t(HAIRPIN + h, MB + "confidence", num(c["confidence"]))

    # -------------------------------------------------------- dead entries
    for d in load("db_dead_mirna"):
        iri = HAIRPIN + d["mirna_acc"].strip()
        t(iri, RDF + "type", f"<{MB}DeadEntry>")
        t(iri, DCT + "identifier", lit(d["mirna_acc"].strip()))
        if d["mirna_id"]:
            t(iri, RDFS + "label", lit(d["mirna_id"].strip()))
        if d["comment"]:
            t(iri, RDFS + "comment", lit(d["comment"]))
        # 77 withdrawn entries "forward" to themselves — a miRBase artifact
        # meaning "no replacement". Emitting it would assert, via the range of
        # mb:forwardTo, that 77 accessions absent from the stem-loop table are
        # stem-loops. Drop the self-reference; it carries no information.
        fwd = (d["forward_to"] or "").strip()
        if fwd and fwd != d["mirna_acc"].strip():
            t(iri, MB + "forwardTo", f"<{HAIRPIN + fwd}>")

    # ------------------------------------------------- transcript context
    for i, c in enumerate(load("db_mirna_context")):
        h = acc_of.get(c["auto_mirna"])
        if not h:
            continue
        iri = f"{HAIRPIN}{h}/context/{i}"
        t(HAIRPIN + h, MB + "transcriptContext", f"<{iri}>")
        t(iri, RDF + "type", f"<{MB}TranscriptContext>")
        if c["transcript_id"]:
            t(iri, MB + "transcriptId", lit(c["transcript_id"]))
        if c["transcript_name"]:
            t(iri, MB + "transcriptName", lit(c["transcript_name"]))
        if c["transcript_source"]:
            t(iri, MB + "transcriptSource", lit(c["transcript_source"]))
        if c["overlap_type"]:
            t(iri, MB + "overlapType", lit(c["overlap_type"]))
        if c["overlap_sense"]:
            t(iri, MB + "overlapSense", lit(c["overlap_sense"]))

    # ================================================ GENOME COORDINATES =====
    # faldo:reference is an OBJECT property — it points at the contig/chromosome
    # resource, not at its name. Reference sequences are minted per
    # (scope, seqid) because names like 'chr1' recur across organisms.
    seen_refseq: set[str] = set()

    def refseq_iri(scope: str, seqid: str, assembly_iri: str | None) -> str:
        safe_scope = quote(scope, safe="")
        iri = f"{REFSEQ}{safe_scope}/{quote(seqid, safe='')}"
        if iri not in seen_refseq:
            seen_refseq.add(iri)
            t(iri, RDF + "type", f"<{MB}ReferenceSequence>")
            t(iri, RDFS + "label", lit(seqid))
            t(iri, DCT + "identifier", lit(seqid))
            if assembly_iri:
                t(iri, MB + "assembly", f"<{assembly_iri}>")
        return iri

    def emit_region(iri: str, subject: str, seqid: str, start, end, strand: str,
                    assembly_iri: str | None, source_iri: str, scope: str) -> None:
        ref = refseq_iri(scope, seqid, assembly_iri)
        t(subject, MB + "location", f"<{iri}>")
        t(iri, RDF + "type", f"<{FALDO}Region>")
        t(iri, MB + "coordinateSource", f"<{source_iri}>")
        t(iri, FALDO + "reference", f"<{ref}>")
        stype = (FALDO + "ForwardStrandPosition" if strand == "+"
                 else FALDO + "ReverseStrandPosition" if strand == "-"
                 else FALDO + "StrandedPosition")
        for role, pos in (("begin", start), ("end", end)):
            p_iri = f"{iri}/{role}"
            t(iri, FALDO + role, f"<{p_iri}>")
            t(p_iri, RDF + "type", f"<{FALDO}ExactPosition>")
            t(p_iri, RDF + "type", f"<{stype}>")
            t(p_iri, FALDO + "position", num(pos))
            t(p_iri, FALDO + "reference", f"<{ref}>")
        if assembly_iri:
            t(iri, MB + "assembly", f"<{assembly_iri}>")

    # 1) the curated, assembly-stamped GFF3 files (stem-loops AND matures)
    headers = {h["organism"]: h for h in load("gff3_headers")}
    for org, h in headers.items():
        if h["genome_build_id"]:
            a_iri = f"{ASSEMBLY}{h['genome_build_id'].replace(' ', '_')}"
            t(a_iri, RDF + "type", f"<{MB}GenomeAssembly>")
            t(a_iri, RDFS + "label", lit(h["genome_build_id"]))
            if h["genome_build_accession"]:
                t(a_iri, MB + "assemblyAccession", lit(h["genome_build_accession"]))

    for g in load("gff3_features"):
        # A mature that sits on several hairpins appears once per copy, and the
        # GFF3 `ID` is then suffixed (MIMAT0027618_1) while `Alias` keeps the
        # real accession. Key the entity off the Alias, and hang the copy-local
        # id and parent on the REGION — the parent differs per copy.
        gff_id = g["attr_id"]
        acc = g["attr_alias"] or gff_id
        if not acc:
            continue
        base = MATURE if acc.startswith("MIMAT") else HAIRPIN
        subject = base + acc
        h = headers.get(g["organism"], {})
        a_iri = (f"{ASSEMBLY}{h['genome_build_id'].replace(' ', '_')}"
                 if h.get("genome_build_id") else None)
        region = f"{subject}/region/{g['organism']}/{g['ordinal']}"
        emit_region(region, subject, g["seqid"], g["start"], g["end"],
                    g["strand"], a_iri, f"{MB}GFF3",
                    h.get("genome_build_id") or g["organism"])
        if gff_id and gff_id != acc:
            t(region, DCT + "identifier", lit(gff_id))
        # the GFF3 `Name=` can lag the published name (the same 8 stale entries)
        if g["attr_name"] and g["attr_name"] != label_of.get(acc):
            t(region, RDFS + "label", lit(g["attr_name"]))
        if g["derives_from"]:
            # on the REGION this is the parent of THIS copy (mb:parentStemLoop);
            # on the mature it is the general mb:derivesFrom
            t(region, MB + "parentStemLoop", f"<{HAIRPIN + g['derives_from']}>")
            t(subject, MB + "derivesFrom", f"<{HAIRPIN + g['derives_from']}>")
            t(HAIRPIN + g["derives_from"], MB + "hasMatureProduct", f"<{subject}>")

    # 2) mirna_chromosome_build — 153 species, far beyond the 31 GFF3 files
    sp_of_hairpin = {m["mirna_acc"]: sp_code.get(m["auto_species"], "")
                     for m in mirna}
    for i, c in enumerate(load("db_mirna_chromosome_build")):
        h = acc_of.get(c["auto_mirna"])
        if not h or c["contig_start"] is None:
            continue
        subject = HAIRPIN + h
        region = f"{subject}/region/build/{i}"
        # no assembly is recorded here, so scope the reference sequence by
        # species — 'supercont1.1' means different things in different genomes
        emit_region(region, subject, c["xsome"] or "", c["contig_start"],
                    c["contig_end"], (c["strand"] or "").strip(), None,
                    f"{MB}ChromosomeBuild", sp_of_hairpin.get(h, "unknown"))


if __name__ == "__main__":
    main()
