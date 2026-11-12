// The small graph the view tests draw: people who know each other, a project, a lab.
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { build } from "rete-graph";

export const PEOPLE = `@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <https://example.org/> .
ex:ada a ex:Person ; rdfs:label "Ada Lovelace" ; ex:knows ex:charles , ex:mary ; ex:worksFor ex:engine .
ex:charles a ex:Person ; rdfs:label "Charles Babbage" ; ex:worksFor ex:engine ; ex:born "1791" .
ex:mary a ex:Person ; rdfs:label "Mary Somerville" ; ex:knows ex:charles .
ex:engine a ex:Project ; rdfs:label "Analytical Engine" .
ex:lab a ex:Org ; rdfs:label "Lab" ; ex:hosts ex:engine .
`;

export async function fixture() {
  const dir = await mkdtemp(join(tmpdir(), "rete-mcpb-view-"));
  await writeFile(join(dir, "people.rete"), await build(PEOPLE, "ttl"));
  return dir;
}
