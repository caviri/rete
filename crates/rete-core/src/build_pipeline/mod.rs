#[allow(dead_code)]
pub(crate) mod ingest;
#[allow(dead_code)]
pub(crate) mod spool;
pub(crate) mod timing;

#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub(crate) enum BuildPipelineError {
    #[error(transparent)]
    Ingest(#[from] crate::ingest::IngestError),
    #[error(transparent)]
    File(#[from] crate::file::FileError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("term id space exceeds u32")]
    TooManyTerms,
    #[error("invalid build spool: {0}")]
    InvalidSpool(&'static str),
    #[error(
        "external build supports the default graph only (named graph {0} found); \
             use the standard build, or strip graph terms (.nq -> .nt) first"
    )]
    NamedGraph(String),
    #[error("build arithmetic overflow: {0}")]
    Overflow(&'static str),
    #[cfg(test)]
    #[error("injected build failure: {0}")]
    InjectedFailure(&'static str),
}
