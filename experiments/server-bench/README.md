# What a SPARQL server uses for the landing graphs

The cost section of the landing page (`site/landing/illustrated.js`) prices a
SPARQL server next to a `.rete` file in a bucket. This folder holds the scripts
that measured those servers, and the results.

The four landing graphs (Linked Jazz, the Hermetic books, the old maps and the
Hugging Face Hub) were each loaded into [QLever](https://github.com/ad-freiburg/qlever)
and [Oxigraph](https://github.com/oxigraph/oxigraph), and both were asked the
landing demo's questions.

## How

- **Input.** The published files on R2, exported with `rete export --format nq`
  (rete-cli 0.7.4). Every graph is default-graph only, so the lines are
  N-Triples. The line counts equal the files' statement counts.
- **Engines.**
  - QLever: image `adfreiburg/qlever:commit-18c097c2fb`, which was `latest` on
    2026-11-14.
  - Oxigraph 0.5.13: its binary, copied out of `ghcr.io/oxigraph/oxigraph:0.5.13`
    and run in `python:3.12-slim`, so the same wrapper can measure it.
- **Machine.** Docker, with 2 CPUs and 4 GB for each container. One engine ran at
  a time. Every machine priced on the page has 2 vCPUs.
- **Memory.** `measure.py` runs the engine as its child and reads
  `getrusage(RUSAGE_CHILDREN).ru_maxrss`: the kernel's peak resident set size.
  For a server it also reads `VmHWM` after the last question, and the larger of
  the two is kept.
- **Disk.** The size of the index directory each engine wrote.
- **Questions.** `client.py` asks each question once right after the server is
  up, then 5 more times. QLever's result cache is cleared before every run.
  Oxigraph has none.
  - Rows are counted from each answer and match what the `.rete` file returns.
  - A CONSTRUCT is counted in distinct triples: QLever can repeat a triple in a
    CONSTRUCT answer.
  - The times are on this machine with the index in memory. They say nothing
    about a cold disk.

Two inputs needed a flag:

- **QLever: `qlever-settings.json`.** One davidrumsey literal is a 38-digit
  `xsd:integer`, which QLever refuses unless told to read overflowing integers
  as doubles.
- **Oxigraph: `--lenient`.** One Hugging Face IRI is `<%20https://…>`, a
  relative IRI that Oxigraph refuses. rete and QLever take it as it is.

With 32 CPUs, Oxigraph's bulk loader ran out of 4 GB on the Hugging Face graph
within 5 s. With 2 it fits.

## Run

```sh
./run.sh                      # all four graphs, both engines
GRAPHS="bph:bph-2026-07-01" ENGINES=qlever ./run.sh
python summarize.py ../../site/landing/servers.js
```

`run.sh` expects:

- a Docker volume `sb-nt` holding `<file>.nt`
- a volume `sb-idx` for the indexes
- a network `sb-net`
- Oxigraph's binary at `bin/oxigraph`

The commands that set these up are at the top of `run.sh`.

## Results

Measured 2026-11-15. Times are medians of 5 runs, in ms, one per question in the
landing's order. Every question returned as many rows as it does from the file,
and every index holds all the statements (`count.sh`, `results/counts.tsv`).

| Graph | Statements | Engine | Build | Build peak | Index | Serving peak | Answers (ms) |
|---|---:|---|---:|---:|---:|---:|---|
| Linked Jazz | 11,471 | QLever | 0.1 s | 40 MB | 0.3 MB | 63 MB | 3.0, 5.4, 3.7, 3.9 |
| Linked Jazz | 11,471 | Oxigraph | 0.3 s | 27 MB | 1.2 MB | 22 MB | 1.0, 6.3, 0.9, 0.7 |
| Hermetic books | 5,741,344 | QLever | 13.7 s | 1,583 MB | 145 MB | 114 MB | 11, 4.3, 6.9 |
| Hermetic books | 5,741,344 | Oxigraph | 12.5 s | 1,915 MB | 652 MB | 70 MB | 19, 20, 33 |
| Old maps | 5,151,281 | QLever | 10.8 s | 1,381 MB | 150 MB | 124 MB | 10, 9.0, 11 |
| Old maps | 5,151,281 | Oxigraph | 11.8 s | 1,849 MB | 521 MB | 62 MB | 71, 12, 908 |
| Hugging Face Hub | 84,650,044 | QLever | 272.4 s | 4,300 MB | 2,219 MB | 112 MB | 3.3, 5.5, 4.8 |
| Hugging Face Hub | 84,650,044 | Oxigraph | 176.4 s | 1,863 MB | 7,251 MB | 84 MB | 2.2, 3.7, 87 |

QLever's Hugging Face build reached the cap (4.3 GB is 4 GiB) and finished.
Oxigraph's needed the 2-CPU limit to fit: with 32 CPUs its loader ran out of
memory. The server peaks are for the demo's questions only.

`results/summary.json` has every figure; the landing reads `site/landing/servers.js`,
which `summarize.py` writes from it.
