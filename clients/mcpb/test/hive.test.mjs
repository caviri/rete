// The hive plot's arithmetic (src/view/hive.js), in Node.
import assert from "node:assert/strict";
import test from "node:test";

import { axisAngle, hiveAxes, hivePositions } from "../src/view/hive.js";

const node = (id, kind = "iri", type = null) => ({ id, kind, type, label: id });
const edge = (source, target) => ({ id: `${source}>${target}`, source, target });

// Two people and two papers: people write papers, a paper cites another and has a title.
const NODES = [node("ada", "iri", "Person"), node("bob", "iri", "Person"), node("p1", "iri", "Paper"), node("p2", "iri", "Paper"), node("lit", "literal")];
const EDGES = [edge("ada", "p1"), edge("ada", "p2"), edge("bob", "p2"), edge("p1", "p2"), edge("p1", "lit")];

test("axes come from the classes when there are two or more, commonest first", () => {
  const { by, axes, axisOf } = hiveAxes(NODES, EDGES);
  assert.equal(by, "type");
  assert.deepEqual(axes.map((a) => [a.key, a.count]), [["Paper", 2], ["Person", 2], ["(untyped)", 1]]);
  assert.equal(axisOf.get("ada"), axisOf.get("bob"));
  assert.notEqual(axisOf.get("ada"), axisOf.get("p1"));
});

test("without classes the axes are the roles: sources, both, sinks", () => {
  const plain = ["a", "b", "c", "d"].map((id) => node(id));
  const { by, axes, axisOf } = hiveAxes(plain, [edge("a", "b"), edge("b", "c"), edge("a", "c")]);
  assert.equal(by, "role");
  assert.deepEqual(axes.map((a) => [a.key, a.count]), [["sources", 1], ["both", 2], ["sinks", 1]], "d has no links and sits with both");
  assert.equal(axisOf.get("a"), 0);
  assert.equal(axisOf.get("c"), 2);
});

test("an axis nobody is on is not drawn", () => {
  const { axes } = hiveAxes([node("a"), node("b")], [edge("a", "b")]);
  assert.deepEqual(axes.map((a) => a.key), ["sources", "sinks"]);
});

test("a graph where every node is alike falls back to kind, then to thirds by links", () => {
  const kinds = hiveAxes([node("a"), node("b", "literal")], [edge("a", "b")], "kind");
  assert.deepEqual(kinds.axes.map((a) => a.key), ["resources", "literals"]);
  // a ring: one in and one out each, so one role; one kind; no classes
  const ring = ["a", "b", "c", "d", "e", "f"].map((id) => node(id));
  const auto = hiveAxes(ring, ring.map((n, i) => edge(n.id, ring[(i + 1) % 6].id)));
  assert.equal(auto.by, "degree");
  assert.deepEqual(auto.axes.map((a) => a.count), [2, 2, 2]);
});

test("more classes than axes: the commonest keep an axis, the rest share the last", () => {
  const many = Array.from({ length: 9 }, (_, i) => node(`n${i}`, "iri", `C${i}`));
  many.push(node("x", "iri", "C0"));
  const { axes } = hiveAxes(many, []);
  assert.equal(axes.length, 6);
  assert.equal(axes.at(-1).key, "other");
  assert.equal(axes[0].key, "C0");
  assert.equal(axes.reduce((s, a) => s + a.count, 0), 10);
});

test("two axes sit a quarter turn apart, three or more evenly, the first pointing up", () => {
  assert.equal(axisAngle(0, 3), -Math.PI / 2);
  assert.ok(Math.abs(axisAngle(1, 2) - axisAngle(0, 2) - Math.PI / 2) < 1e-9);
  assert.ok(Math.abs(axisAngle(1, 3) - axisAngle(0, 3) - (2 * Math.PI) / 3) < 1e-9);
});

test("nodes run out along their axis in order of links, name breaking ties", () => {
  const { pos, axes } = hivePositions(NODES, EDGES, { by: "type", order: "degree" });
  const radius = (p, id) => Math.hypot(p[id].x, p[id].y);
  // ada has two links, bob one: ada is further out
  assert.ok(radius(pos, "ada") > radius(pos, "bob"));
  // p1 has three links (ada, p2, lit), p2 three (ada, bob, p1): a tie, so by name p1 is inside
  assert.ok(radius(pos, "p2") > radius(pos, "p1"));
  const byName = hivePositions(NODES, EDGES, { by: "type", order: "name" });
  assert.ok(radius(byName.pos, "bob") > radius(byName.pos, "ada"), "by name, bob after ada");
  const byOut = hivePositions(NODES, EDGES, { by: "type", order: "out" });
  assert.ok(radius(byOut.pos, "ada") > radius(byOut.pos, "bob"), "ada has two links out, bob one");
  // every node is on its axis, between its ends
  const person = axes.find((a) => a.key === "Person");
  for (const id of ["ada", "bob"]) {
    const r = radius(pos, id);
    assert.ok(r >= person.from - 1e-9 && r <= person.to + 1e-9);
    assert.ok(Math.abs(Math.atan2(pos[id].y, pos[id].x) - person.angle) < 1e-9);
  }
  for (const a of axes) assert.ok(a.to > a.from && a.from > 0);
});

test("edges bow toward the angle between their axes; edges along one axis arc to one side", () => {
  const plan = hivePositions(NODES, EDGES, { by: "type" });
  // the control point of the curve, from the bow along the normal (-dy, dx)
  const control = (e) => {
    const s = plan.pos[e.source];
    const t = plan.pos[e.target];
    const len = Math.hypot(t.x - s.x, t.y - s.y);
    const d = plan.bow[e.id];
    return { x: (s.x + t.x) / 2 + (-(t.y - s.y) / len) * d, y: (s.y + t.y) / 2 + ((t.x - s.x) / len) * d };
  };
  // an edge between two axes bows outward, away from the centre, like the curves of a hive plot
  const c = control(edge("ada", "p1"));
  const m = { x: (plan.pos.ada.x + plan.pos.p1.x) / 2, y: (plan.pos.ada.y + plan.pos.p1.y) / 2 };
  assert.ok(Math.hypot(c.x, c.y) > Math.hypot(m.x, m.y), "the control point is further out than the middle of the chord");
  const at = (key) => plan.axes.find((a) => a.key === key).angle;
  // p1 -> p2 stay on the Paper axis: the control point is off the axis, to its clockwise side
  const same = control(edge("p1", "p2"));
  const paper = at("Paper");
  const across = -Math.sin(paper) * same.x + Math.cos(paper) * same.y;
  assert.ok(across > 5, `an arc beside the axis (${across})`);
  assert.ok(plan.bow["ada>p1"] !== undefined && plan.bow["p1>lit"] !== undefined);
});

test("what cannot be drawn is skipped, not thrown on", () => {
  const plan = hivePositions([node("a"), node("b")], [edge("a", "a"), edge("a", "ghost"), edge("a", "b")]);
  assert.deepEqual(Object.keys(plan.bow), ["a>b"]);
  assert.deepEqual(hivePositions([], []).pos, {});
  const one = hivePositions([node("solo")], []);
  assert.ok(Number.isFinite(one.pos.solo.x) && Number.isFinite(one.pos.solo.y));
});
