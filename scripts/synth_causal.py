#!/usr/bin/env python3
"""Synthesize the playground's causal graph -> examples/causal.nt (N-Triples).

A small but structurally rich *cardiometabolic* causal model. It is hand-curated
(every edge means something) but emitted from data here so the IRIs stay
consistent and the planted defects are explicit and documented.

What's in it, and what each part is for:

  * A typed factor hierarchy (RiskFactor / Condition / Disease / Symptom /
    Outcome / Treatment, all rdfs:subClassOf ex:Factor) so SHACL can target by
    class and `:causes` endpoints are checkable.
  * `ex:causes` (an owl:TransitiveProperty) wiring up the discoverable shapes the
    SPARQL examples in catalog.js explore:
      - confounders  (a fork: Z -> X, Z -> Y; e.g. Poverty)
      - mediators    (a chain: X -> M -> Y; the metabolic steps)
      - colliders    (a join: X -> C <- Y; e.g. chest pain)
      - feedback loops (cycles: ?x :causes+ ?x; the metabolic and stress loops)
      - root causes / terminal outcomes
  * `ex:reduces` — the protective relation (intervention -> risk factor it lowers).
  * Node attributes (`ex:prevalence` xsd:decimal, `ex:modifiable` xsd:boolean,
    `ex:evidence` one of ex:established/probable/hypothesized) for SHACL value
    checks, with three INTENTIONAL data defects planted (see DEFECT comments).
  * The original coherence defects, PRESERVED so the Coherence tab / `rete reason`
    demo and scripts/smoke.sh keep working:
      - Tier-0 (schema):  ex:Relapsed is rdfs:subClassOf BOTH ex:HealthyState and
        ex:DiseaseState, which are owl:disjointWith -> ex:Relapsed is UNSATISFIABLE.
      - Instance:         ex:p is typed as both states -> a disjoint-class clash.

Run (deterministic):  python3 scripts/synth_causal.py
"""

import pathlib

EX = "http://ex/"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
SUBCLASS = "http://www.w3.org/2000/01/rdf-schema#subClassOf"
LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
DISJOINT = "http://www.w3.org/2002/07/owl#disjointWith"
TRANSITIVE = "http://www.w3.org/2002/07/owl#TransitiveProperty"
XSD = "http://www.w3.org/2001/XMLSchema#"

OUT = pathlib.Path(__file__).resolve().parent.parent / "examples" / "causal.nt"


def ex(name: str) -> str:
    return f"<{EX}{name}>"


def lit_dec(v: str) -> str:
    return f'"{v}"^^<{XSD}decimal>'


def lit_bool(v: bool) -> str:
    return f'"{str(v).lower()}"^^<{XSD}boolean>'


def lit_str(v: str) -> str:
    return f'"{v}"'


# ---- the factor type hierarchy (everything rolls up to ex:Factor) -----------
SUBCLASSES = [
    ("RiskFactor", "Factor"),
    ("Condition", "Factor"),
    ("Disease", "Condition"),
    ("Symptom", "Factor"),
    ("Treatment", "Factor"),
    ("Outcome", "Factor"),
]

# ---- nodes: (local, type, label, {prevalence, modifiable, evidence}) --------
# DEFECTs are planted in three risk factors so the "Risk factors are well
# described" SHACL shape flags exactly three rows.
NODES = [
    # risk factors (exogenous drivers)
    ("Poverty", "RiskFactor", "Poverty",
     {"prevalence": "0.12", "evidence": "probable"}),                 # DEFECT: no ex:modifiable
    ("Genetics", "RiskFactor", "Genetic predisposition",
     {"prevalence": "0.20", "modifiable": False, "evidence": "established"}),
    ("Aging", "RiskFactor", "Aging",
     {"prevalence": "0.16", "modifiable": False, "evidence": "established"}),
    ("Smoking", "RiskFactor", "Smoking",
     {"prevalence": "0.18", "modifiable": True, "evidence": "established"}),
    ("PoorDiet", "RiskFactor", "Poor diet",
     {"prevalence": "0.30", "modifiable": True, "evidence": "established"}),
    ("PhysicalInactivity", "RiskFactor", "Physical inactivity",
     {"prevalence": "0.25", "modifiable": True, "evidence": "established"}),
    ("Stress", "RiskFactor", "Chronic stress",
     {"prevalence": "1.4", "modifiable": True, "evidence": "probable"}),  # DEFECT: prevalence > 1
    ("AirPollution", "RiskFactor", "Air pollution",
     {"prevalence": "0.40", "modifiable": False, "evidence": "rumored"}),  # DEFECT: evidence off-list

    # intermediate conditions (mediators)
    ("Obesity", "Condition", "Obesity",
     {"prevalence": "0.13", "evidence": "established"}),
    ("Hypertension", "Condition", "Hypertension",
     {"prevalence": "0.30", "evidence": "established"}),
    ("HighCholesterol", "Condition", "High LDL cholesterol",
     {"prevalence": "0.28", "evidence": "established"}),
    ("InsulinResistance", "Condition", "Insulin resistance",
     {"prevalence": "0.22", "evidence": "probable"}),
    ("Inflammation", "Condition", "Chronic inflammation",
     {"evidence": "probable"}),
    ("PoorSleep", "Condition", "Poor sleep",
     {"evidence": "probable"}),
    ("Anxiety", "Condition", "Anxiety",
     {"evidence": "probable"}),

    # diseases
    ("Atherosclerosis", "Disease", "Atherosclerosis",
     {"evidence": "established"}),
    ("Diabetes", "Disease", "Type 2 diabetes",
     {"prevalence": "0.10", "evidence": "established"}),
    ("CoronaryArteryDisease", "Disease", "Coronary artery disease",
     {"prevalence": "0.06", "evidence": "established"}),
    ("ChronicKidneyDisease", "Disease", "Chronic kidney disease",
     {"evidence": "established"}),

    # symptoms
    ("ChestPain", "Symptom", "Chest pain", {}),
    ("Breathlessness", "Symptom", "Breathlessness", {}),
    ("Fatigue", "Symptom", "Fatigue", {}),

    # terminal outcomes
    ("MyocardialInfarction", "Outcome", "Heart attack (MI)", {}),
    ("Stroke", "Outcome", "Stroke", {}),
    ("Death", "Outcome", "Death", {}),

    # treatments (protective interventions)
    ("Statins", "Treatment", "Statins", {}),
    ("Exercise", "Treatment", "Exercise", {}),
    ("Metformin", "Treatment", "Metformin", {}),
    ("BPMedication", "Treatment", "Antihypertensives", {}),
    ("SmokingCessation", "Treatment", "Smoking cessation", {}),
]

# ---- ex:causes edges (risk causation; transitive) ---------------------------
CAUSES = [
    # roots & confounders (one driver -> several downstream factors)
    ("Poverty", "Smoking"), ("Poverty", "PoorDiet"),
    ("Poverty", "PhysicalInactivity"), ("Poverty", "Stress"),
    ("Genetics", "HighCholesterol"), ("Genetics", "Hypertension"),
    ("Genetics", "Diabetes"),
    ("Aging", "Hypertension"), ("Aging", "Atherosclerosis"),
    ("Aging", "HighCholesterol"),
    ("AirPollution", "Inflammation"), ("AirPollution", "Atherosclerosis"),

    # behaviour -> metabolic conditions
    ("Smoking", "Atherosclerosis"), ("Smoking", "Inflammation"),
    ("Smoking", "CoronaryArteryDisease"),
    ("PoorDiet", "Obesity"), ("PoorDiet", "HighCholesterol"),
    ("PhysicalInactivity", "Obesity"), ("PhysicalInactivity", "InsulinResistance"),

    # the metabolic feedback loop: Obesity -> Inflammation -> InsulinResistance -> Obesity
    ("Obesity", "Inflammation"), ("Obesity", "InsulinResistance"),
    ("Obesity", "Hypertension"),
    ("Inflammation", "InsulinResistance"), ("Inflammation", "Atherosclerosis"),
    ("InsulinResistance", "Diabetes"), ("InsulinResistance", "Obesity"),

    # cholesterol / hypertension -> vascular damage
    ("HighCholesterol", "Atherosclerosis"),
    ("Hypertension", "Atherosclerosis"), ("Hypertension", "Stroke"),
    ("Hypertension", "ChronicKidneyDisease"),

    # atherosclerosis -> cardiovascular disease -> events
    ("Atherosclerosis", "CoronaryArteryDisease"), ("Atherosclerosis", "Stroke"),
    ("CoronaryArteryDisease", "MyocardialInfarction"),
    ("CoronaryArteryDisease", "ChestPain"),
    ("CoronaryArteryDisease", "Breathlessness"),

    # diabetes complications
    ("Diabetes", "ChronicKidneyDisease"), ("Diabetes", "Atherosclerosis"),
    ("Diabetes", "Fatigue"),

    # terminal outcomes -> death
    ("MyocardialInfarction", "Death"), ("Stroke", "Death"),
    ("ChronicKidneyDisease", "Death"),

    # anxiety arm -> shared (collider) symptoms
    ("Stress", "Anxiety"), ("Anxiety", "ChestPain"),
    ("Anxiety", "Breathlessness"), ("Anxiety", "PoorSleep"),

    # the stress / sleep feedback loop
    ("Stress", "PoorSleep"), ("PoorSleep", "Stress"), ("PoorSleep", "Fatigue"),
]

# ---- ex:reduces edges (protective: intervention -> factor it lowers) --------
REDUCES = [
    ("Statins", "HighCholesterol"), ("Statins", "Atherosclerosis"),
    ("Exercise", "Obesity"), ("Exercise", "Hypertension"),
    ("Exercise", "InsulinResistance"), ("Exercise", "Inflammation"),
    ("Metformin", "InsulinResistance"), ("Metformin", "Diabetes"),
    ("BPMedication", "Hypertension"),
    ("SmokingCessation", "Smoking"),
]


def main() -> None:
    out = []
    w = out.append

    w("# Cardiometabolic causal model for the rete playground.")
    w("# Generated by scripts/synth_causal.py -- edit there, not here.")
    w("# Prefix: ex: = <http://ex/>.  ex:causes is an owl:TransitiveProperty,")
    w("# so SPARQL `ex:causes+` walks whole pathways; ex:reduces is protective.")
    w("")

    w("# --- factor type hierarchy --------------------------------------------------")
    for c, parent in SUBCLASSES:
        w(f"{ex(c)} <{SUBCLASS}> {ex(parent)} .")
    w(f"{ex('causes')} <{RDF_TYPE}> <{TRANSITIVE}> .")
    w("")

    w("# --- states (disjoint) + PRESERVED coherence defects ------------------------")
    w("# HealthyState and DiseaseState are disjoint; Relapsed is a subclass of both")
    w("# (Tier-0 unsatisfiable) and patient :p is typed as both (instance clash).")
    w(f"{ex('HealthyState')} <{DISJOINT}> {ex('DiseaseState')} .")
    w(f"{ex('Relapsed')} <{SUBCLASS}> {ex('HealthyState')} .")
    w(f"{ex('Relapsed')} <{SUBCLASS}> {ex('DiseaseState')} .")
    w(f"{ex('p')} <{RDF_TYPE}> {ex('HealthyState')} .")
    w(f"{ex('p')} <{RDF_TYPE}> {ex('DiseaseState')} .")
    w("")

    w("# --- nodes: type, label, attributes (3 planted DEFECTs, see synth_causal.py) -")
    for local, typ, label, attrs in NODES:
        w(f"{ex(local)} <{RDF_TYPE}> {ex(typ)} .")
        w(f"{ex(local)} <{LABEL}> {lit_str(label)} .")
        if "prevalence" in attrs:
            w(f"{ex(local)} {ex('prevalence')} {lit_dec(attrs['prevalence'])} .")
        if "modifiable" in attrs:
            w(f"{ex(local)} {ex('modifiable')} {lit_bool(attrs['modifiable'])} .")
        if "evidence" in attrs:
            w(f"{ex(local)} {ex('evidence')} {ex(attrs['evidence'])} .")
    w("")

    w("# --- ex:causes edges (transitive risk causation) ----------------------------")
    for s, o in CAUSES:
        w(f"{ex(s)} {ex('causes')} {ex(o)} .")
    w("")

    w("# --- ex:reduces edges (protective interventions) ----------------------------")
    for s, o in REDUCES:
        w(f"{ex(s)} {ex('reduces')} {ex(o)} .")

    text = "\n".join(out) + "\n"
    OUT.write_text(text, encoding="utf-8")
    n_triples = sum(1 for ln in out if ln and not ln.startswith("#"))
    print(f"synth_causal: wrote {OUT} ({n_triples} triples)")


if __name__ == "__main__":
    main()
