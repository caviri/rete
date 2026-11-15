// The layered drawing's positions (src/view/layout.js), in Node.
import assert from "node:assert/strict";
import test from "node:test";

import { layeredPositions } from "../src/view/layout.js";

const box = (id, w, h) => ({ id, w, h });

test("parallel edges, which made dagre throw, still lay out in layers", () => {
  // The smallest part of the MTG schema that did it: Battle → Color by three
  // predicates, with these box sizes (measured in the view).
  const nodes = [box("Battle", 113, 171), box("Color", 113, 66), box("Class", 113, 66), box("Supertype", 113, 66), box("Rarity", 113, 41)];
  const edge = (source, target, p) => ({ id: `${source} ${p} ${target}`, source, target });
  const edges = [
    edge("Battle", "Color", "colors"),
    edge("Battle", "Color", "colorIdentity"),
    edge("Battle", "Color", "producedMana"),
    edge("Class", "Color", "colors"),
    edge("Battle", "Supertype", "supertype"),
    edge("Class", "Rarity", "rarity"),
  ];
  const { pos, error } = layeredPositions(nodes, edges);
  assert.equal(error, null);
  assert.ok(pos.Battle.y < pos.Color.y && pos.Class.y < pos.Color.y, "sources above their targets");
  for (const n of nodes) assert.ok(Number.isFinite(pos[n.id].x) && Number.isFinite(pos[n.id].y), n.id);
});

test("what is not linked is packed in rows under the layers", () => {
  const { pos } = layeredPositions([box("a", 100, 50), box("b", 100, 50), box("lone", 80, 40)], [{ id: "e", source: "a", target: "b" }]);
  assert.ok(pos.lone.y > pos.b.y, "below the layered part");
});
