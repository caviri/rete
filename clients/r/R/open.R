#' Open a .rete graph
#'
#' Opens a `.rete` file for SPARQL querying: a local path, an `http(s)://`
#' URL, or a raw vector holding a complete file image. Local and remote opens
#' are lazy — remote files are read over HTTP range requests, fetching only
#' the byte ranges each query touches (the host must honor `Range`).
#'
#' @param source A file path, an `http(s)://` URL, or a raw vector.
#' @return A `rete_graph` object.
#' @examples
#' \dontrun{
#' g <- rete_open("https://data.graphplaza.com/boe/boe.rete")
#' rete_query(g, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5")
#' }
#' @export
rete_open <- function(source) {
  if (is.raw(source)) {
    ptr <- RGraph$from_bytes(source)
    label <- "<raw>"
  } else if (is.character(source) && length(source) == 1L) {
    if (grepl("^https?://", source)) {
      ptr <- RGraph$from_url(source)
      label <- source
    } else {
      ptr <- RGraph$from_path(path.expand(source))
      label <- source
    }
  } else {
    stop("`source` must be a single path, an http(s) URL, or a raw vector")
  }
  structure(list(ptr = ptr, source = label), class = "rete_graph")
}

#' @export
print.rete_graph <- function(x, ...) {
  info <- jsonlite::fromJSON(x$ptr$info())
  cat(sprintf(
    "<rete graph> %s\n  %s quads, %s terms, %s pyramid level(s), %s named graph(s)\n",
    x$source,
    format(info$quads, big.mark = ","), format(info$terms, big.mark = ","),
    info$pyramidLevels, info$namedGraphs
  ))
  invisible(x)
}

#' Graph size and header counts
#'
#' @param graph A `rete_graph`.
#' @return A list with `quads`, `terms`, `pyramidLevels`, and `namedGraphs`.
#' @export
rete_info <- function(graph) {
  stopifnot(inherits(graph, "rete_graph"))
  jsonlite::fromJSON(graph$ptr$info())
}

#' Physical fetch counters
#'
#' Cumulative bytes and range requests actually fetched since the graph was
#' opened — the number that shows how lazy a remote query really is.
#'
#' @param graph A `rete_graph`.
#' @return A list with `fileLength`, `bytes`, and `requests`.
#' @export
rete_stats <- function(graph) {
  stopifnot(inherits(graph, "rete_graph"))
  jsonlite::fromJSON(graph$ptr$stats())
}

#' Content hash
#'
#' @param graph A `rete_graph`.
#' @return The file's blake3-16 content hash as a hex string.
#' @export
rete_content_hash <- function(graph) {
  stopifnot(inherits(graph, "rete_graph"))
  graph$ptr$content_hash()
}
