# Star joins

A **star** is several triple patterns on one subject:

```sparql
SELECT ?flag (SUM(?price * (1 - ?discount)) AS ?revenue) WHERE {
  ?line :returnFlag ?flag ;
        :extendedPrice ?price ;
        :discount ?discount ;
        :shipDate ?date .
  FILTER(?date <= "1998-09-02"^^xsd:date)
}
GROUP BY ?flag
```

It is the shape of a table row: one entity, some of its properties. Reports,
aggregates and anything that reads like SQL are made of stars, joined to each
other through a key or a link.

Since 0.7.10 the engine joins a star in one pass. This page says how, when the
rule applies, and what was measured. It changes how queries are planned, not the
file: every `.rete` built by an earlier release is read as before, and answers
do not change.

## One merge instead of a join per pattern

A file with the six permutations stores each predicate's triples sorted by
subject (PSO). The patterns of a star are therefore streams already in the same
order, and joining them is a merge: advance every stream to the next subject
they all have, emit its rows, go on. No hash table is built and nothing is
looked up per row. Memory is the rows of the current subject.

Before 0.7.10 only the first two patterns were merged. Each further pattern was
scanned whole into a relation and hash-joined to the rows so far, or, above two
million rows, looked up in the index once per row. A star of six predicates
over 600,572 subjects took 3 s; it takes 0.04 s.

The merge is taken when:

- at least two patterns have the same subject variable and a constant
  predicate, and share no other variable with each other;
- each is estimated above 65,536 triples (below that, a hash table is cheap
  and small graphs keep their plans);
- none is far larger than the first (a merge reads every member end to end);
- the file has the permutation that sorts them by subject. A file built with
  `--permutations 3` does not, and keeps the previous plan.

A star inside a larger join is merged the same way and then joined to the rest
as one relation. A query that is nothing but a star streams its rows as they
are merged; with other patterns around it, a star below two million rows is
built first, because looking the other patterns up once per star row was
measured slower than hashing them (17 s against 2.6 s on one join). A grouped query on [several threads](sparql.md#threads) slices
the star by subject range, the same range on every member.

## Scan a small pattern instead of probing it

The second change is about the step between two stars. When the rows found so
far are few next to the next pattern, each row looks that pattern up in the
index ([join method](sparql.md#evaluation-model)). The rule assumed a lookup
costs a few microseconds. It does when the row binds the subject: one short
range of SPO. It does not when the row binds only the object of a large
predicate: joining 20,000 parts to the 600,572 line items that reference them
took 6.6 s by lookup, 330 µs each, against 0.3 s to scan that predicate once
and hash-join it.

So a pattern of at most about a million triples is now scanned once when more
than 512 rows ask about it, they are more than a 512th of it, and they do not
bind its subject. Larger patterns keep the lookup: scanning them costs memory.

## Measurements

TPC-H at scale factor 0.1 as RDF: 866,602 rows, one subject per row, one
predicate per column, 12.5 M triples, a 134 MB file built by 0.7.8 and not
rebuilt. The queries are TPC-H's, written as SPARQL. One process, the file
open, best of two runs, 4 GB container.

| query | shape | 0.7.8 | 0.7.10 |
|---|---|---:|---:|
| Q1 | one star of 6, filter, group | 5.45 s | 0.31 s |
| Q6 | one star of 4, filters | 1.47 s | 0.04 s |
| Q3 | 3 stars joined | 7.46 s | 0.33 s |
| Q5 | 6 stars joined | 4.91 s | 0.33 s |
| Q10 | 4 stars joined | 1.50 s | 0.47 s |
| Q12 | 2 stars joined | 3.32 s | 0.85 s |
| Q14 | 2 stars joined | 9.67 s | 0.68 s |
| Q19 | 2 stars joined | 8.00 s | 0.26 s |

Peak memory of Q1 went from 1.6 GB to 0.3 GB. The answers of all eight, and of
four point lookups, equal DuckDB's on the same data as Parquet. DuckDB on that
Parquet answers each in 0.03 to 0.09 s: a column store is still several times
faster on this shape.

## Ordering by what a pattern adds

The order of the patterns used to be smallest first. After the first pattern it
is now the one that adds the fewest rows to each row already joined: its size
divided by the measured number of distinct subjects or objects of its
predicate, for each variable already bound. Two stars sharing two variables
were joined on one and filtered on the other afterwards, 7 M intermediate rows
and more than 4 GB for an answer of 1,332 rows; that query takes 0.1 s.

A join whose intermediate result is measured above two million rows streams
from its first pattern instead of being built in memory. Only measured sizes
count: an estimate for a pattern with a constant subject or object is a whole
tile and can be off by thousands, so such a join keeps its plan.

## Checked against main

Every figure below compares main without these changes to main with them, on
files that were not rebuilt.

- **Eight local graphs, 466 queries** (each file's own card queries plus
  generated stars, joins, groups, OPTIONAL and DISTINCT on its classes;
  TPC-H, INE, Figshare, OpenAlex, ecosyste.ms papers, Wikidata paintings,
  OpenCitations and CORDIS): 460 answered by both, all 460 the same. Two OpenAlex stars
  that ran out of 3.2 GB now answer in 0.1 s. Four run out of memory or time
  on both. 99 queries are at least 1.5 times faster and none is that much
  slower; lazy reads went from 2,351 to 1,616.
- **The 695 example queries of the playground catalog, on the published files
  over HTTP**: 687 answered by both. 681 give the same rows. The other six
  end in `ORDER BY ... LIMIT` over ties, or build a `GROUP_CONCAT` whose order
  is not defined; without the `LIMIT`, and with the concatenated values
  sorted, both engines give the same answer for all six. Eight time out or
  run out of memory on both. Requests 17,080 to 17,113, bytes 5.64 GB to
  5.69 GB: most catalog examples are selective lookups and are planned as
  before.

## Known limits

- A two-hop join over two large predicates with no selective end still builds
  its intermediate result, and can run out of memory.
- A file built with `--permutations 3` gets none of the star merge.

## What it does not change

- Single-pattern aggregates were already answered from the index's own counts
  and are as fast as before.
- A query with a small `LIMIT`, an `ASK`, or a selective constant still starts
  from the selective end and looks the rest up.
- Nothing proportional to a large pattern is held on the streaming path; the
  star merge holds less than the join it replaces.
- Still not a cost-based optimizer over the whole algebra: patterns are ordered
  greedily, one at a time.
