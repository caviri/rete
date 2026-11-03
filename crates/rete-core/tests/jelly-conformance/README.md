# Jelly conformance cases (subset)

Every third case of the "from Jelly" suites of Jelly-RDF 1.1 (`v1_1`, row
layout) and 1.2 (`v1_2`, column layout), plus the first negative case of each
category. They come from <https://github.com/Jelly-RDF/jelly-protobuf>
(`test/rdf`, commit 987cd30c), which is Apache-2.0 licensed: see `LICENSE` here.
The files are unmodified.

`jelly_conformance.rs` runs them. To run the whole suite instead, point
`RETE_JELLY_CONFORMANCE` at a checkout's `test/rdf` directory.
