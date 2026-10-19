"""Build-time GUARD for datacite.rete: refuse a build whose identity or counts drifted.

Run by build_rete.sh after `emit` (and after `rete build`), before the file is moved
into place. Exit 1, naming every failed check, when:

  - a record stated a single-valued property twice (guard|functional_violation.*);
  - a subject IRI was emitted as a record twice (guard|record_iri_emitted_twice), or
    the number of distinct record IRIs differs from the plan's DOI count;
  - the records per snapshot differ from the plan (R9);
  - the PidRelations + events without one + dropped events do not account for every
    PID Links event the plan kept (R18: one PidRelation per uuid);
  - the plan's registered exceptions differ from the ones emit used (R0);
  - the Dataset Card the build embedded is not the card this tally produces
    (make_metadata.py --tally), i.e. the card's counts are not the file's.

The plan itself already refused to run on an identity disagreement or an IRI collision
between two registered DOIs (parquet_to_nt.check_identity, build_link_plan).

Usage: python3 guard.py --plan OUT/plan.parquet --tally OUT/datacite.tally.json [--card CARD]
"""
import argparse
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def total(tally, rule, what, sets=None):
    n = 0
    for st, per in tally["counts"].items():
        if sets and not any(st.startswith(s) for s in sets):
            continue
        n += per.get(rule, {}).get(what, 0)
    return n


def check(plan_stats, plan_exceptions, tally, card=None):
    fails, info = [], {}
    c = tally["counts"]
    g = c.get("all", {}).get("guard", {})
    for st, per in c.items():
        for what, n in per.get("guard", {}).items():
            if what.startswith("functional_violation") and n:
                fails.append(f"{st}: {n} records state {what.split('.', 1)[1]} more than once")
    if g.get("record_iri_emitted_twice"):
        fails.append(f"{g['record_iri_emitted_twice']} record IRIs emitted twice, e.g. "
                     f"{tally['examples'].get('all|guard|record_iri_emitted_twice')}")
    recs = sum(total(tally, "R9", f"records.from_{y}") for y in ("2023", "2024", "2025"))
    info["records_emitted"] = recs
    if "records_emitted" not in g:
        fails.append("no record-IRI guard ran (emit without --guard-dir)")
    elif g.get("record_iris_distinct") != plan_stats.get("dois") or g["records_emitted"] != recs:
        fails.append(f"record IRIs: {g.get('records_emitted')} emitted, {g.get('record_iris_distinct')} "
                     f"distinct, plan has {plan_stats.get('dois')} DOIs, tally {recs}")
    for y in ("2023", "2024", "2025"):
        got, want = total(tally, "R9", f"records.from_{y}"), plan_stats.get(f"records_from_{y}", 0)
        if got != want:
            fails.append(f"records from {y}: emitted {got}, plan {want}")
    link_rows = sum(per.get("input", {}).get("rows", 0) for st, per in c.items() if st.startswith("links-"))
    rel = total(tally, "R18", "pid_relation")
    no_uuid = total(tally, "R18", "dropped.pid_relation.no_uuid")
    dropped = total(tally, "links", "dropped.event")
    info.update(link_events=link_rows, pid_relations=rel, events_dropped=dropped, events_without_uuid=no_uuid)
    if link_rows and link_rows != plan_stats.get("link_events_distinct"):
        fails.append(f"PID Links events read {link_rows}, plan kept {plan_stats.get('link_events_distinct')}")
    if rel + no_uuid + dropped != link_rows:
        fails.append(f"PidRelations {rel} + without uuid {no_uuid} + dropped {dropped} != events {link_rows}")
    if sorted(plan_exceptions) != sorted(tally.get("registered_exceptions", [])):
        fails.append("registered exceptions differ between plan and emit")
    if card is not None:
        sys.path.insert(0, HERE)
        import make_metadata
        want = make_metadata.rete_card(tally)
        if json.loads(json.dumps(want)) != card:
            diff = [k for k in set(want) | set(card) if want.get(k) != card.get(k)]
            fails.append(f"the embedded Dataset Card is not this build's: fields {sorted(diff)} differ; "
                         f"run make_metadata.py --tally <this tally>, commit the card, rebuild")
    return fails, info


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--plan", required=True)
    ap.add_argument("--tally", required=True)
    ap.add_argument("--card", help="the card file the build embedded (omit for a dry run)")
    a = ap.parse_args(argv)
    with open(a.plan + ".json") as fh:
        stats = json.load(fh)
    exceptions = stats.pop("registered_exceptions_list", [])
    with open(a.tally, encoding="utf-8") as fh:
        tally = json.load(fh)
    card = None
    if a.card:
        with open(a.card, encoding="utf-8") as fh:
            card = json.load(fh)
    fails, info = check(stats, exceptions, tally, card)
    print(json.dumps({"guard": "FAIL" if fails else "PASS", "failures": fails, **info}, indent=1))
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
