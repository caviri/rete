#' Run a SPARQL query
#'
#' Evaluates a SPARQL query against the graph. SELECT queries return a
#' `data.frame` with one column per variable — IRI brackets are stripped and
#' typed literals are coerced (`xsd:integer` to integer/double,
#' `xsd:double`/`decimal`/`float` to double, `xsd:boolean` to logical); other
#' terms come back as character. ASK returns a logical scalar;
#' CONSTRUCT/DESCRIBE return a `data.frame` with `subject`, `predicate`,
#' `object` columns. Use [rete_query_raw()] for the unprocessed result
#' envelope with full term fidelity.
#'
#' @param graph A `rete_graph` from [rete_open()].
#' @param query A SPARQL query string.
#' @param reason If `TRUE`, answers include OWL 2 QL entailment, computed by
#'   query rewriting over the file's ontology (works on remote files too).
#' @return A `data.frame`, or a logical scalar for ASK.
#' @export
rete_query <- function(graph, query, reason = FALSE) {
  env <- rete_query_raw(graph, query, reason = reason)
  switch(env$kind,
    ask = env$boolean,
    select = select_to_df(env),
    construct = construct_to_df(env),
    stop("unexpected result kind: ", env$kind)
  )
}

#' Raw SPARQL result envelope
#'
#' @inheritParams rete_query
#' @return The engine's JSON result envelope, parsed to a list. Terms stay in
#'   their N-Triples token form (`<iri>`, `"literal"^^<datatype>`, `_:bnode`).
#' @export
rete_query_raw <- function(graph, query, reason = FALSE) {
  stopifnot(inherits(graph, "rete_graph"), is.character(query), length(query) == 1L)
  jsonlite::fromJSON(graph$ptr$query(query, isTRUE(reason)), simplifyVector = FALSE)
}

select_to_df <- function(env) {
  vars <- vapply(env$vars, identity, character(1))
  rows <- env$rows
  columns <- lapply(vars, function(v) {
    tokens <- vapply(rows, function(row) {
      value <- row[[v]]
      if (is.null(value)) NA_character_ else value
    }, character(1))
    coerce_terms(tokens)
  })
  names(columns) <- vars
  as.data.frame(columns, optional = TRUE, stringsAsFactors = FALSE, check.names = FALSE)
}

construct_to_df <- function(env) {
  triples <- env$triples
  df <- data.frame(
    subject = coerce_terms(vapply(triples, function(t) t[[1]], character(1))),
    predicate = coerce_terms(vapply(triples, function(t) t[[2]], character(1))),
    object = coerce_terms(vapply(triples, function(t) t[[3]], character(1))),
    stringsAsFactors = FALSE
  )
  df
}

XSD <- "http://www.w3.org/2001/XMLSchema#"
INT_TYPES <- paste0(XSD, c(
  "integer", "long", "int", "short", "byte",
  "nonNegativeInteger", "nonPositiveInteger", "negativeInteger",
  "positiveInteger", "unsignedLong", "unsignedInt", "unsignedShort", "unsignedByte"
))
DBL_TYPES <- paste0(XSD, c("decimal", "double", "float"))

# Split one N-Triples token into c(value, datatype); language tags and IRI
# brackets are resolved into the value.
parse_term <- function(token) {
  if (is.na(token)) {
    return(c(NA_character_, NA_character_))
  }
  if (startsWith(token, "<") && endsWith(token, ">") && !startsWith(token, "<<")) {
    return(c(substr(token, 2L, nchar(token) - 1L), NA_character_))
  }
  if (startsWith(token, "\"")) {
    chars <- strsplit(token, "", fixed = TRUE)[[1]]
    i <- 2L
    while (i <= length(chars)) {
      if (chars[i] == "\\") {
        i <- i + 2L
      } else if (chars[i] == "\"") {
        break
      } else {
        i <- i + 1L
      }
    }
    lex <- unescape_nt(substr(token, 2L, i - 1L))
    rest <- substr(token, i + 1L, nchar(token))
    if (startsWith(rest, "^^<") && endsWith(rest, ">")) {
      return(c(lex, substr(rest, 4L, nchar(rest) - 1L)))
    }
    return(c(lex, NA_character_)) # plain or @lang literal
  }
  c(token, NA_character_) # bnode / quoted triple / anything else
}

unescape_nt <- function(s) {
  if (!grepl("\\\\", s)) {
    return(s)
  }
  s <- gsub("\\\\t", "\t", s)
  s <- gsub("\\\\n", "\n", s)
  s <- gsub("\\\\r", "\r", s)
  s <- gsub("\\\\\"", "\"", s)
  gsub("\\\\\\\\", "\\\\", s)
}

# One column of tokens -> the most useful R vector.
coerce_terms <- function(tokens) {
  parsed <- vapply(tokens, parse_term, character(2))
  values <- parsed[1L, ]
  datatypes <- parsed[2L, ]
  known <- datatypes[!is.na(datatypes)]
  if (length(known) > 0L && all(known %in% INT_TYPES)) {
    ints <- suppressWarnings(as.integer(values))
    if (!anyNA(ints[!is.na(values)])) {
      return(ints)
    }
    return(suppressWarnings(as.numeric(values)))
  }
  if (length(known) > 0L && all(known %in% c(INT_TYPES, DBL_TYPES))) {
    return(suppressWarnings(as.numeric(values)))
  }
  if (length(known) > 0L && all(known == paste0(XSD, "boolean"))) {
    return(values == "true")
  }
  unname(values)
}

#' Label prefix search
#'
#' @param graph A `rete_graph`.
#' @param prefix The label prefix to complete.
#' @param limit Maximum number of hits.
#' @return A `data.frame` with `label` and `subject` columns.
#' @export
rete_prefix_search <- function(graph, prefix, limit = 20L) {
  stopifnot(inherits(graph, "rete_graph"))
  hits <- jsonlite::fromJSON(graph$ptr$prefix_search(prefix, as.integer(limit)),
    simplifyVector = FALSE
  )
  data.frame(
    label = vapply(hits, function(h) h$label, character(1)),
    subject = coerce_terms(vapply(hits, function(h) h$subject, character(1))),
    stringsAsFactors = FALSE
  )
}

#' Full-text search
#'
#' Requires a file built with the opt-in text index.
#'
#' @param graph A `rete_graph`.
#' @param words Words to search for (a character vector, or one string that
#'   is split on whitespace).
#' @param contains Optional substring filter.
#' @param limit Maximum number of subjects.
#' @return A character vector of subject IRIs.
#' @export
rete_text_search <- function(graph, words, contains = NULL, limit = 100L) {
  stopifnot(inherits(graph, "rete_graph"))
  if (length(words) == 1L) {
    words <- strsplit(words, "\\s+")[[1]]
  }
  tokens <- graph$ptr$text_search(words, if (is.null(contains)) "" else contains, as.integer(limit))
  coerce_terms(tokens)
}

#' Class and predicate profile
#'
#' @param graph A `rete_graph`.
#' @return A list with `classes` (`data.frame`: class, instances) and
#'   `relations` (`data.frame`: subject_class, predicate, object_class, count).
#' @export
rete_schema <- function(graph) {
  stopifnot(inherits(graph, "rete_graph"))
  env <- jsonlite::fromJSON(graph$ptr$schema(), simplifyVector = FALSE)
  classes <- data.frame(
    class = coerce_terms(vapply(env$classes, function(x) x[[1]], character(1))),
    instances = vapply(env$classes, function(x) as.integer(x[[2]]), integer(1)),
    stringsAsFactors = FALSE
  )
  relations <- data.frame(
    subject_class = coerce_terms(vapply(env$relations, function(x) x[[1]], character(1))),
    predicate = coerce_terms(vapply(env$relations, function(x) x[[2]], character(1))),
    object_class = coerce_terms(vapply(env$relations, function(x) x[[3]], character(1))),
    count = vapply(env$relations, function(x) as.integer(x[[4]]), integer(1)),
    stringsAsFactors = FALSE
  )
  list(classes = classes, relations = relations)
}

#' The embedded Dataset Card
#'
#' @param graph A `rete_graph`.
#' @return The card as a list, or `NULL` when the file carries none. On lazy
#'   opens only the metadata section's byte range is fetched.
#' @export
rete_card <- function(graph) {
  stopifnot(inherits(graph, "rete_graph"))
  raw <- graph$ptr$card()
  if (!nzchar(raw)) {
    return(NULL)
  }
  jsonlite::fromJSON(raw, simplifyVector = FALSE)
}

#' Example SPARQL queries embedded in the file
#'
#' Reads the starter queries a `.rete` carries in its Dataset Card. Every
#' `sparql` entry runs as-is via [rete_query()].
#'
#' @param graph A `rete_graph`.
#' @return A `data.frame` with `title`, `question`, `tier`, and `sparql`
#'   columns (`NA` for plain legacy entries), or an empty one.
#' @export
rete_examples <- function(graph) {
  card <- rete_card(graph)
  rich <- if (is.null(card)) list() else card$queries
  legacy <- if (is.null(card)) list() else card$example_queries
  chr <- function(x) if (is.null(x)) NA_character_ else x
  rows <- c(
    lapply(rich, function(q) {
      data.frame(
        title = chr(q$title), question = chr(q$question),
        tier = chr(q$tier), sparql = q$sparql, stringsAsFactors = FALSE
      )
    }),
    lapply(legacy, function(q) {
      data.frame(
        title = NA_character_, question = NA_character_,
        tier = NA_character_, sparql = q, stringsAsFactors = FALSE
      )
    })
  )
  if (length(rows) == 0L) {
    return(data.frame(
      title = character(), question = character(),
      tier = character(), sparql = character(), stringsAsFactors = FALSE
    ))
  }
  do.call(rbind, rows)
}
