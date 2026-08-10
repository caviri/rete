NT <- paste(
  '<http://example.org/alice> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/Person> .',
  '<http://example.org/alice> <http://www.w3.org/2000/01/rdf-schema#label> "Alice \\"the researcher\\"" .',
  '<http://example.org/alice> <http://example.org/age> "42"^^<http://www.w3.org/2001/XMLSchema#integer> .',
  '<http://example.org/bob> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/Person> .',
  '<http://example.org/bob> <http://www.w3.org/2000/01/rdf-schema#label> "Bob"@en .',
  '<http://example.org/bob> <http://example.org/knows> <http://example.org/alice> .',
  sep = "\n"
)

build_graph <- function(...) rete_open(rete_build(NT, ...))

test_that("build + open + SELECT returns a coerced data.frame", {
  g <- build_graph()
  expect_s3_class(g, "rete_graph")
  expect_equal(rete_info(g)$quads, 6)

  df <- rete_query(g, "SELECT ?s ?o WHERE { ?s <http://example.org/knows> ?o }")
  expect_s3_class(df, "data.frame")
  expect_equal(nrow(df), 1)
  expect_equal(df$s, "http://example.org/bob") # IRI brackets stripped
  expect_equal(df$o, "http://example.org/alice")

  age <- rete_query(g, "SELECT ?age WHERE { <http://example.org/alice> <http://example.org/age> ?age }")
  expect_identical(age$age, 42L) # xsd:integer -> integer

  labels <- rete_query(g, "SELECT ?l WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l }")
  expect_true('Alice "the researcher"' %in% labels$l) # NT escapes resolved
  expect_true("Bob" %in% labels$l) # @en tag dropped from the value
})

test_that("ASK and CONSTRUCT", {
  g <- build_graph()
  expect_true(rete_query(g, "ASK { ?s <http://example.org/knows> ?o }"))
  expect_false(rete_query(g, "ASK { ?s <http://example.org/hates> ?o }"))

  df <- rete_query(g, paste(
    "CONSTRUCT { ?o <http://example.org/knownBy> ?s }",
    "WHERE { ?s <http://example.org/knows> ?o }"
  ))
  expect_equal(nrow(df), 1)
  expect_equal(df$predicate, "http://example.org/knownBy")
})

test_that("local file opens lazily and matches the raw image", {
  path <- tempfile(fileext = ".rete")
  data <- rete_build(NT)
  writeBin(as.vector(data), path)
  g <- rete_open(path)
  expect_equal(rete_info(g)$quads, 6)
  expect_equal(nchar(rete_content_hash(g)), 32)
  expect_gte(rete_stats(g)$requests, 1)
})

test_that("dataset card and embedded examples round-trip", {
  g <- build_graph(
    card = list(
      title = "Tiny people graph",
      license = "CC0-1.0",
      example_queries = list("SELECT ?s WHERE { ?s ?p ?o }")
    ),
    text_index = TRUE
  )
  card <- rete_card(g)
  expect_equal(card$title, "Tiny people graph")
  expect_equal(card$quad_count, 6)
  expect_gte(card$format_version, 1)

  ex <- rete_examples(g)
  expect_equal(nrow(ex), 1)
  df <- rete_query(g, ex$sparql[[1]])
  expect_gt(nrow(df), 0)

  hits <- rete_text_search(g, "researcher")
  expect_true("http://example.org/alice" %in% hits)
})

test_that("derive_card is opt-in, and derives what the CLI derives", {
  # Default: curated fields only. `rete-graph`'s R twin is published too, so an
  # existing call must keep writing the bytes it always wrote.
  plain <- rete_card(build_graph(card = list(title = "Curated only")))
  expect_null(plain$predicates)
  expect_null(plain$queries)
  expect_null(plain$signals)

  # Opt in: the whole auto-derived profile, from the code `rete build --card`
  # runs — plus the CLI's write-time validation of the curated half.
  g <- build_graph(
    card = list(title = "Derived", keywords = list("zeta", "alpha")),
    derive_card = TRUE
  )
  card <- rete_card(g)
  expect_equal(card$top_n, 100)
  expect_gt(length(card$predicates), 0)
  expect_gt(length(card$classes), 0)
  expect_gt(length(card$vocabularies), 0)
  # Canonicalized (sorted + deduplicated), exactly as `--card-file` does it.
  expect_equal(unlist(card$keywords), c("alpha", "zeta"))

  # Every generated starter query is runnable on the file that carries it.
  ex <- rete_examples(g)
  expect_gt(nrow(ex), 5)
  for (q in ex$sparql) expect_no_error(rete_query(g, q))

  # And the CLI's card rules now apply: a free-text theme is refused, with the
  # CLI's own wording.
  expect_error(
    rete_build(NT, card = list(theme = list("physics")), derive_card = TRUE),
    "not an IRI"
  )
})

test_that("schema profile and prefix search have the right shapes", {
  g <- build_graph()
  s <- rete_schema(g)
  classes <- setNames(s$classes$instances, s$classes$class)
  expect_equal(unname(classes["http://example.org/Person"]), 2L)
  hits <- rete_prefix_search(g, "Ali")
  expect_s3_class(hits, "data.frame")
  expect_named(hits, c("label", "subject"))
})

test_that("no card means NULL and empty examples", {
  g <- build_graph()
  expect_null(rete_card(g))
  expect_equal(nrow(rete_examples(g)), 0)
})

test_that("remote graph over HTTP range (live)", {
  skip_on_cran()
  skip_if_offline("data.graphplaza.com")
  g <- rete_open("https://data.graphplaza.com/boe/boe.rete")
  df <- rete_query(g, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 3")
  expect_equal(nrow(df), 3)
  s <- rete_stats(g)
  expect_lt(s$bytes, s$fileLength) # lazy, not a download
  expect_gt(nrow(rete_examples(g)), 0) # boe ships starter queries
})
