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
#' @return A raw vector holding the file image — pass it to [rete_open()] or
#'   write it with `writeBin()`.
#' @examples
#' nt <- '<urn:a> <urn:knows> <urn:b> .'
#' g <- rete_open(rete_build(nt, card = list(title = "Tiny demo")))
#' rete_query(g, "SELECT ?o WHERE { <urn:a> <urn:knows> ?o }")
#' @export
rete_build <- function(text, format = "nt", card = NULL,
                       pyramid = c("louvain", "types", "none"),
                       text_index = FALSE) {
  pyramid <- match.arg(pyramid)
  card_json <- if (is.null(card)) {
    ""
  } else {
    if (!is.list(card)) stop("`card` must be a named list")
    as.character(jsonlite::toJSON(card, auto_unbox = TRUE))
  }
  build_dataset(text, format, card_json, pyramid, isTRUE(text_index))
}
