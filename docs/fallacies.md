# Experiment — fallacy graphs from a conversation

An agent connected to the [rete MCP server](agents.md) can now **author**
knowledge graphs, not just read them: search existing vocabularies, draft
an ontology, lint it until clean, build a real `.rete`, and validate it —
all inside one chat. This page documents the flow with a worked example:
**annotating the fallacies in a spoken conversation**. Everything below ran
verbatim against the server (the outputs are real).

The tools involved: `suggest_vocabulary` → `check_ontology` → `build_rete`
→ `sparql_query` / `validate_shacl`.

## The scenario

Someone sends a voice note of a budget debate. The host app (ChatGPT does
this natively) transcribes it:

> **Bruno:** We should raise the maintenance budget: breakdowns grew 40%.
> **Ana:** We can't trust Bruno with anything budget-related — he failed
> economics twice.
> **Ana:** I propose reviewing the support contracts before deciding.
> **Bruno:** So Ana wants to freeze everything and let the factory fall
> apart.

Two fallacies hide in there: an *ad hominem* (Ana attacks Bruno, not his
claim) and a *straw man* (Bruno refutes a distorted version of Ana's
proposal). The goal: a queryable, validated graph of that structure.

## 1. Discover before minting

`suggest_vocabulary("argument fallacy premise")` searches
[Linked Open Vocabularies](https://lov.linkeddata.es) and surfaces the
**Argument Interchange Format** (AIF) — so the ontology anchors to it
instead of reinventing "argument":

## 2. Draft the ontology, lint until clean

The agent drafts Turtle and calls `check_ontology`. A deliberately broken
draft shows what the linter catches:

```json
{"ok": false, "issues": [
  {"severity": "warning", "message": "…fallacy#Utterancia is used as rdfs:domain/rdfs:range but never declared in this document…"},
  {"severity": "error",   "message": "…fallacy#text is declared BOTH owl:ObjectProperty and owl:DatatypeProperty"},
  {"severity": "warning", "message": "…fallacy#Fallacy has no rdfs:label — agents and humans will see a bare IRI"}
]}
```

The battery: strict parse, domains/ranges over undeclared classes,
dangling `subClassOf` targets, missing labels/comments, object+datatype
clashes, subclass cycles, and an **OWL 2 QL rewriter smoke test**. The
fixed ontology (abridged — full version in the collapsible below) comes
back `{"ok": true, "profile": {"classes": 7, "properties": 4, "reasoner": "ok"}}`:

```ttl
@prefix aif: <http://www.arg.dundee.ac.uk/aif#> .
@prefix fal: <https://w3id.org/rete/fallacy#> .

fal:Utterance a owl:Class ; rdfs:subClassOf aif:I-node ; rdfs:label "Utterance" .
fal:Fallacy   a owl:Class ; rdfs:label "Fallacy" .
fal:AdHominem a owl:Class ; rdfs:subClassOf fal:Fallacy ; rdfs:label "Ad hominem" ;
  rdfs:comment "Attacks the speaker instead of the claim." .
fal:StrawMan  a owl:Class ; rdfs:subClassOf fal:Fallacy ; rdfs:label "Straw man" .

fal:saidBy  a owl:ObjectProperty  ; rdfs:domain fal:Utterance ; rdfs:range fal:Speaker .
fal:text    a owl:DatatypeProperty ; rdfs:domain fal:Utterance ; rdfs:range xsd:string .
fal:quotes  a owl:ObjectProperty  ; rdfs:domain fal:Fallacy ; rdfs:range fal:Utterance ;
  rdfs:comment "The utterance that commits the fallacy." .
fal:attacks a owl:ObjectProperty  ; rdfs:domain fal:Fallacy ; rdfs:range fal:Utterance ;
  rdfs:comment "The utterance the fallacious move targets." .
```

## 3. Instances with provenance

Each intervention becomes an `Utterance` (text + speaker); each annotation
a `Fallacy` node pointing at what it quotes and what it attacks:

```ttl
ex:u1 a fal:Utterance ; fal:saidBy ex:bruno ; fal:text "We should raise the maintenance budget…" .
ex:u2 a fal:Utterance ; fal:saidBy ex:ana   ; fal:text "We can't trust Bruno — he failed economics twice." .

ex:f1 a fal:AdHominem ; fal:quotes ex:u2 ; fal:attacks ex:u1 .
ex:f2 a fal:StrawMan  ; fal:quotes ex:u4 ; fal:attacks ex:u3 .
```

## 4. Build a real, queryable file

`build_rete(ontology + instances, card={title: "Fallacy analysis — Ana &
Bruno budget debate", …}, examples=[…])` returns in under a second:

```json
{"dataset": "generated/f2fd70100e1a0a52.rete", "url": "/generated/f2fd70100e1a0a52.rete",
 "bytes": 5609, "quads": 65, "content_hash": "…"}
```

That 5.6 KB file is a complete `.rete` — dictionary, indexes, pyramid,
Dataset Card with the embedded example queries — served at an ephemeral
URL, range-readable by every client and the playground, and **immediately
usable by the other tools** via its dataset key.

## 5. Query it — the reasoning payoff

No instance is typed `fal:Fallacy` directly, only as subclasses. Plain vs
reasoned, same query:

```sparql
SELECT (COUNT(DISTINCT ?f) AS ?n) WHERE { ?f a fal:Fallacy }
# reason=false → 0        reason=true → 2
```

(Note the `DISTINCT`: query rewriting derives `ex:f1` three ways — via the
subclass AND via the domains of `quotes` and `attacks` — and a bare COUNT
counts every derivation.) And the analysis query that the file itself
carries as an example:

```
Ana   → fal:AdHominem
Bruno → fal:StrawMan
```

## 6. SHACL as the annotation contract

"Every annotated fallacy must cite the utterance that commits it and the
one it attacks":

```ttl
[] a sh:NodeShape ;
  sh:targetClass fal:AdHominem ; sh:targetClass fal:StrawMan ; sh:targetClass fal:FalseDilemma ;
  sh:property [ sh:path fal:quotes  ; sh:minCount 1 ;
    sh:message "every annotated fallacy must cite the utterance that commits it" ] ;
  sh:property [ sh:path fal:attacks ; sh:minCount 1 ] .
```

`validate_shacl` → `conforms: true`. Tighten the contract (require a
`fal:severity` the annotations don't carry) and it reports exactly the two
expected violations — the loop an agent uses to keep its own annotations
honest.

## Same tooling, other conversations

Nothing here is fallacy-specific — the pattern is *speech → ontology →
instances → validated graph*:

- **meeting minutes** → decisions, owners, deadlines, dissent;
- **oral-history interviews** → people/places/events, federable with the
  historical datasets already in the catalog;
- **spoken symptom descriptions** → instances against a lightweight
  clinical vocabulary;
- **full debate argument maps** — AIF proper, with rebuttals and support
  edges.

Generated files are ephemeral (they live until the next Space restart) —
ask for `include_base64` to keep the file, or graduate it through the
[publish workflow](dataset-cards.md) when it deserves a permanent home.
