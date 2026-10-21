//! Where a command's query text comes from: the command line, or a file — or
//! standard input — named by `--query-file`.
//!
//! The command line cannot carry a large query. The OS caps it: one argument
//! may be at most 128 KiB on Linux (`MAX_ARG_STRLEN`), and the whole command
//! line 32,767 UTF-16 code units on Windows. A query with a few thousand IRIs
//! in a `VALUES` block crosses either, and fails before rete ever runs (OpenAIRE
//! QA, E5). Every command that takes a query takes `--query-file <path>` too,
//! `-` meaning standard input; Clap makes the two mutually exclusive.

use std::io::Read as _;

use anyhow::Context as _;

/// The query to run: the inline one, else the contents of `file` (`-` reads
/// standard input). `None` only when neither was given — the commands whose
/// query is required have Clap refuse that first.
pub(crate) fn read_optional(
    inline: Option<String>,
    file: Option<&str>,
) -> anyhow::Result<Option<String>> {
    if let Some(query) = inline {
        return Ok(Some(query));
    }
    let Some(path) = file else {
        return Ok(None);
    };
    let text = if path == "-" {
        let mut bytes = Vec::new();
        std::io::stdin()
            .read_to_end(&mut bytes)
            .context("cannot read the query from standard input (--query-file -)")?;
        String::from_utf8(bytes)
            .map_err(|_| anyhow::anyhow!("the query on standard input is not UTF-8"))?
    } else {
        let bytes =
            std::fs::read(path).with_context(|| format!("cannot read --query-file {path}"))?;
        String::from_utf8(bytes)
            .map_err(|_| anyhow::anyhow!("--query-file {path} is not UTF-8 text"))?
    };
    // A byte-order mark (what some Windows editors put first) is not SPARQL.
    let text = match text.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_string(),
        None => text,
    };
    if text.trim().is_empty() {
        let source = if path == "-" {
            "standard input".to_string()
        } else {
            format!("--query-file {path}")
        };
        anyhow::bail!("the query from {source} is empty");
    }
    Ok(Some(text))
}

/// [`read_optional`] for a command whose query is required.
pub(crate) fn read(inline: Option<String>, file: Option<&str>) -> anyhow::Result<String> {
    read_optional(inline, file)?.ok_or_else(|| {
        anyhow::anyhow!(
            "no query: pass it as an argument or with --query-file <path> (- for stdin)"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_wins_and_a_file_is_read_whole() {
        assert_eq!(read(Some("ASK {}".into()), None).unwrap(), "ASK {}");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("q.rq");
        // A query far past the 128 KiB a Linux argument may carry.
        let big = format!(
            "SELECT * WHERE {{ VALUES ?x {{ {} }} }}",
            (0..20_000)
                .map(|i| format!("<http://example.test/{i}>"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert!(big.len() > 128 * 1024);
        std::fs::write(&path, &big).unwrap();
        assert_eq!(read(None, path.to_str()).unwrap(), big);
    }

    #[test]
    fn bom_empty_missing_and_non_utf8_files() {
        let dir = tempfile::tempdir().unwrap();
        let bom = dir.path().join("bom.rq");
        std::fs::write(&bom, "\u{feff}ASK {}").unwrap();
        assert_eq!(read(None, bom.to_str()).unwrap(), "ASK {}");

        let empty = dir.path().join("empty.rq");
        std::fs::write(&empty, " \n").unwrap();
        let err = read(None, empty.to_str()).unwrap_err().to_string();
        assert!(err.contains("is empty"), "{err}");

        let missing = dir.path().join("missing.rq");
        let err = format!("{:#}", read(None, missing.to_str()).unwrap_err());
        assert!(err.contains("cannot read --query-file"), "{err}");

        let latin1 = dir.path().join("latin1.rq");
        std::fs::write(&latin1, b"ASK { ?s ?p \"caf\xe9\" }").unwrap();
        let err = read(None, latin1.to_str()).unwrap_err().to_string();
        assert!(err.contains("not UTF-8"), "{err}");

        assert!(read_optional(None, None).unwrap().is_none());
        assert!(read(None, None).is_err());
    }
}
