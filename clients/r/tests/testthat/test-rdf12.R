# RDF 1.2 in the R client: reading RDF 1.2 Turtle/TriG, the card's
# quoted-triple signal, and the property blank-node labelling must keep —
# two separate parses never share a label. (The R client has no RDF text
# export, so there is no writer half to test here.)

RDF <- "http://www.w3.org/1999/02/22-rdf-syntax-ns#"

TTL12 <- paste(
  "@prefix ex: <http://example.test/> .",
  "ex:alice ex:says <<( ex:bob ex:knows ex:carol )>> .",
  "<< ex:bob ex:knows ex:dave >> ex:source ex:wiki .",
  'ex:erin ex:knows ex:frank {| ex:since "2020" |} .',
  'ex:gina ex:name "Gina"@en--ltr .',
  sep = "\n"
)

TRIG12 <- paste(
  "@prefix ex: <http://example.test/> .",
  "ex:g {",
  "  ex:alice ex:says <<( ex:bob ex:knows ex:carol )>> .",
  "  << ex:bob ex:knows ex:dave >> ex:source ex:wiki .",
  "}",
  sep = "\n"
)

TTL_STAR <- paste(
  "@prefix ex: <http://example.test/> .",
  "ex:claim ex:states << ex:a ex:p ex:b >> .",
  sep = "\n"
)

NT_QUOTED <- paste(
  "<http://ex/a> <http://ex/p> <http://ex/b> .",
  "<http://ex/claim> <http://ex/states> << <http://ex/a> <http://ex/p> <http://ex/b> >> .",
  sep = "\n"
)

PLAIN <- "<http://ex/a> <http://ex/p> <http://ex/b> ."

ANON <- '[] <http://example.test/p> "x" .'
SUBJECTS <- 'SELECT ?s WHERE { ?s <http://example.test/p> "x" }'

test_that("RDF 1.2 Turtle is read under quoted_triple_syntax = 'rdf12'", {
  g <- rete_open(rete_build(TTL12, "ttl", quoted_triple_syntax = "rdf12"))
  expect_equal(rete_info(g)$quads, 7)
  reifiers <- rete_query(g, paste0("SELECT ?r WHERE { ?r <", RDF, "reifies> ?t }"))
  expect_equal(nrow(reifiers), 2)
  expect_equal(length(unique(reifiers$r)), 2)
  expect_true(rete_query(g, paste(
    "ASK { <http://example.test/erin> <http://example.test/knows>",
    "<http://example.test/frank> }"
  )))
})

test_that("RDF 1.2 TriG is read, named graph and all", {
  g <- rete_open(rete_build(TRIG12, "trig", quoted_triple_syntax = "rdf12"))
  expect_equal(rete_info(g)$quads, 3)
  expect_equal(rete_info(g)$namedGraphs, 1)
})

test_that("rdf-star stays the default, and the flag is validated", {
  expect_error(rete_build(TTL12, "ttl"), "rdf12")
  expect_error(rete_build(PLAIN, quoted_triple_syntax = "rdf13"))
  expect_identical(rete_build(PLAIN), rete_build(PLAIN, quoted_triple_syntax = "rdf12"))
  # The same Turtle bytes are two graphs under the two readers.
  expect_equal(rete_info(rete_open(rete_build(TTL_STAR, "ttl")))$quads, 1)
  expect_equal(
    rete_info(rete_open(rete_build(TTL_STAR, "ttl", quoted_triple_syntax = "rdf12")))$quads,
    2
  )
})

test_that("the card reports quoted triples from the header", {
  q <- rete_card(rete_open(rete_build(NT_QUOTED, card = list(title = "t"))))$signals$quoted_triples
  expect_true(q$present)
  expect_equal(unlist(q$export_surfaces), c("rdf12", "rdf-star"))
  expect_equal(q$export_default, "rdf12")

  plain <- rete_card(rete_open(rete_build(PLAIN, card = list(title = "t"))))
  expect_false(plain$signals$quoted_triples$present)
  expect_null(plain$signals$quoted_triples$export_surfaces)

  # Measured, never stored: the file's own bytes do not carry it.
  bytes <- rete_build(NT_QUOTED, card = list(title = "t"))
  expect_length(grepRaw("quoted_triples", bytes, fixed = TRUE), 0)
  # A file without a card still has none.
  expect_null(rete_card(rete_open(rete_build(NT_QUOTED))))
})

test_that("blank nodes of separate parses stay distinct when merged", {
  labels <- unlist(lapply(c("rdf12", "rdf12", "rdf-star", "rdf-star"), function(syntax) {
    g <- rete_open(rete_build(ANON, "ttl", quoted_triple_syntax = syntax))
    rete_query(g, SUBJECTS)$s
  }))
  expect_length(labels, 4)
  expect_true(all(startsWith(labels, "_:")))
  expect_length(unique(labels), 4)

  merged <- paste0(labels, ' <http://example.test/p> "x" .', collapse = "\n")
  g <- rete_open(rete_build(merged))
  expect_length(unique(rete_query(g, SUBJECTS)$s), 4)
})
