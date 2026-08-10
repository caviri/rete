#' Build a .rete file image from RDF text
#'
#' Assembles a complete, immutable `.rete` file — dictionary, triple indexes,
#' pyramid, and optionally an embedded Dataset Card — from RDF text. Handy
#' for tests and small graphs; for large datasets use the `rete build` CLI,
#' which streams and compresses.
#'
#' @param text RDF text: N-Triples (`"nt"`), N-Quads (`"nq"` — named graphs
#'   become a dataset), Turtle (`"ttl"`), or RDF/XML (`"rdfxml"`).
#' @param format The RDF serialization of `text`.
#' @param card Optional Dataset Card as a named list (curated fields such as
#'   `title`, `description`, `license`, `created`, `example_queries`; counts
#'   are stamped automatically). Read it back with [rete_card()].
#' @param pyramid Community pyramid algorithm: `"louvain"` (topological,
#'   default), `"types"` (one community per `rdf:type` class), or `"none"`.
#' @param text_index If `TRUE`, add the opt-in full-text word index that
#'   powers [rete_text_search()].
#' @param derive_card If `TRUE`, also compute the card's **auto-derived
#'   profile** — predicate and class histograms, vocabularies, datatypes,
#'   languages, the class-link quotient, top hubs, the affordance signals and
#'   the tiered starter-query library — exactly as `rete build --card` does.
#'   `FALSE` (the default) writes the curated fields only, so an existing call
#'   keeps producing the bytes it always produced; derivation walks the graph
#'   twice more, which is a cost to opt into. Turning it on also holds the
#'   curated half to the CLI's write-time rules (reserved top level, `theme`
#'   must be a controlled-vocabulary IRI, the `extra` bag is bounded).
#' @return A raw vector holding the file image — pass it to [rete_open()] or
#'   write it with `writeBin()`.
#' @examples
#' nt <- '<urn:a> <urn:knows> <urn:b> .'
#' g <- rete_open(rete_build(nt, card = list(title = "Tiny demo")))
#' rete_query(g, "SELECT ?o WHERE { <urn:a> <urn:knows> ?o }")
#' @export
rete_build <- function(text, format = "nt", card = NULL,
                       pyramid = c("louvain", "types", "none"),
                       text_index = FALSE, derive_card = FALSE) {
  pyramid <- match.arg(pyramid)
  card_json <- if (is.null(card)) {
    ""
  } else {
    if (!is.list(card)) stop("`card` must be a named list")
    as.character(jsonlite::toJSON(card, auto_unbox = TRUE))
  }
  build_dataset(text, format, card_json, pyramid, isTRUE(text_index),
                isTRUE(derive_card))
}
