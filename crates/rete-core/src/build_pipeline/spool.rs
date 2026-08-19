use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::Triple;

use super::BuildPipelineError;

const TRIPLE_RECORD_BYTES: usize = 12;
const MAX_BLOCK_RECORDS: usize = 1 << 20;

/// A private, uniquely-created spill directory which removes only the directory
/// it created. Files are always closed before callers can replay them, which
/// keeps cleanup compatible with Windows file-handle rules.
#[derive(Clone)]
pub(crate) struct BuildTemp {
    inner: Arc<BuildTempInner>,
}

struct BuildTempInner {
    parent: PathBuf,
    owned: PathBuf,
    cleanup: bool,
}

impl BuildTemp {
    pub(crate) fn new(parent: &Path) -> Result<Self, BuildPipelineError> {
        std::fs::create_dir_all(parent)?;
        let parent = std::fs::canonicalize(parent)?;
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        for _ in 0..1024 {
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let owned = parent.join(format!(".rete-build-{}-{seq}", std::process::id()));
            match std::fs::create_dir(&owned) {
                Ok(()) => {
                    return Ok(Self {
                        inner: Arc::new(BuildTempInner {
                            parent,
                            owned,
                            cleanup: true,
                        }),
                    })
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(BuildPipelineError::InvalidSpool(
            "could not allocate a unique temporary directory",
        ))
    }

    pub(crate) fn path(&self, name: &str) -> Result<PathBuf, BuildPipelineError> {
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
        {
            return Err(BuildPipelineError::InvalidSpool("uncontained spool path"));
        }
        let path = self.inner.owned.join(name);
        if !path.starts_with(&self.inner.owned) {
            return Err(BuildPipelineError::InvalidSpool("uncontained spool path"));
        }
        Ok(path)
    }

    #[cfg(test)]
    pub(crate) fn owned_path(&self) -> &Path {
        &self.inner.owned
    }

    #[cfg(test)]
    pub(crate) fn adopt_existing_for_resume(owned: PathBuf) -> Self {
        Self {
            inner: Arc::new(BuildTempInner {
                parent: owned.parent().unwrap_or(Path::new("")).to_path_buf(),
                owned,
                cleanup: false,
            }),
        }
    }
}

impl Drop for BuildTempInner {
    fn drop(&mut self) {
        if self.cleanup
            && self
                .owned
                .parent()
                .is_some_and(|parent| parent == self.parent)
            && self.owned.is_dir()
        {
            let _ = std::fs::remove_dir_all(&self.owned);
        }
    }
}

/// A replayable canonical `(subject, predicate, object)` record stream.
pub(crate) enum TripleSpool {
    Resident(Vec<Triple>),
    File {
        path: PathBuf,
        count: u64,
        _session: BuildTemp,
    },
}

impl TripleSpool {
    pub(crate) fn write_file(
        temp: &BuildTemp,
        name: &str,
        triples: &[Triple],
    ) -> Result<Self, BuildPipelineError> {
        let count = u64::try_from(triples.len())
            .map_err(|_| BuildPipelineError::Overflow("triple spool count"))?;
        let path = temp.path(name)?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut writer = BufWriter::new(file);
        for &(subject, predicate, object) in triples {
            writer.write_all(&subject.to_le_bytes())?;
            writer.write_all(&predicate.to_le_bytes())?;
            writer.write_all(&object.to_le_bytes())?;
        }
        writer.flush()?;
        drop(writer);
        Ok(Self::File {
            path,
            count,
            _session: temp.clone(),
        })
    }

    pub(crate) fn from_file(
        temp: &BuildTemp,
        path: PathBuf,
        count: u64,
    ) -> Result<Self, BuildPipelineError> {
        let expected = count
            .checked_mul(TRIPLE_RECORD_BYTES as u64)
            .ok_or(BuildPipelineError::Overflow("triple spool byte length"))?;
        let actual = std::fs::metadata(&path)?.len();
        if actual % TRIPLE_RECORD_BYTES as u64 != 0 {
            return Err(BuildPipelineError::InvalidSpool("partial triple record"));
        }
        if actual != expected {
            return Err(BuildPipelineError::InvalidSpool(
                "triple spool length does not match count",
            ));
        }
        Ok(Self::File {
            path,
            count,
            _session: temp.clone(),
        })
    }

    pub(crate) fn count(&self) -> u64 {
        match self {
            Self::Resident(triples) => triples.len() as u64,
            Self::File { count, .. } => *count,
        }
    }

    pub(crate) fn for_each_block(
        &self,
        max_records: usize,
        visit: &mut dyn FnMut(&[Triple]) -> Result<(), BuildPipelineError>,
    ) -> Result<(), BuildPipelineError> {
        let records = max_records.min(MAX_BLOCK_RECORDS);
        if records == 0 {
            return Err(BuildPipelineError::InvalidSpool("zero spool block size"));
        }
        match self {
            Self::Resident(triples) => {
                for block in triples.chunks(records) {
                    visit(block)?;
                }
                Ok(())
            }
            Self::File { path, count, .. } => {
                let expected = count
                    .checked_mul(TRIPLE_RECORD_BYTES as u64)
                    .ok_or(BuildPipelineError::Overflow("triple spool byte length"))?;
                let actual = std::fs::metadata(path)?.len();
                if actual % TRIPLE_RECORD_BYTES as u64 != 0 {
                    return Err(BuildPipelineError::InvalidSpool("partial triple record"));
                }
                if actual != expected {
                    return Err(BuildPipelineError::InvalidSpool(
                        "triple spool length does not match count",
                    ));
                }
                let mut reader = BufReader::new(File::open(path)?);
                let mut remaining = *count;
                let mut block = Vec::with_capacity(records);
                while remaining != 0 {
                    block.clear();
                    let take = usize::try_from(remaining).unwrap_or(records).min(records);
                    for _ in 0..take {
                        let mut record = [0u8; TRIPLE_RECORD_BYTES];
                        reader.read_exact(&mut record).map_err(|error| {
                            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                                BuildPipelineError::InvalidSpool("partial triple record")
                            } else {
                                error.into()
                            }
                        })?;
                        block.push((
                            u32::from_le_bytes(record[0..4].try_into().map_err(|_| {
                                BuildPipelineError::InvalidSpool("subject record width")
                            })?),
                            u32::from_le_bytes(record[4..8].try_into().map_err(|_| {
                                BuildPipelineError::InvalidSpool("predicate record width")
                            })?),
                            u32::from_le_bytes(record[8..12].try_into().map_err(|_| {
                                BuildPipelineError::InvalidSpool("object record width")
                            })?),
                        ));
                    }
                    remaining -= u64::try_from(take)
                        .map_err(|_| BuildPipelineError::Overflow("spool block count"))?;
                    visit(&block)?;
                }
                Ok(())
            }
        }
    }

    pub(crate) const fn is_file_backed(&self) -> bool {
        matches!(self, Self::File { .. })
    }

    pub(crate) fn file_path(&self) -> Option<&Path> {
        match self {
            Self::File { path, .. } => Some(path),
            Self::Resident(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;

    use crate::Triple;

    use super::{BuildTemp, TripleSpool};

    fn triples() -> Vec<Triple> {
        vec![(3, 2, 1), (7, 11, 13), (17, 19, 23), (29, 31, 37)]
    }

    fn test_parent(name: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let parent = std::env::temp_dir().join(format!(
            "rete-build-pipeline-{name}-{}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir(&parent).unwrap();
        parent
    }

    fn collect_blocks(spool: &TripleSpool, max_records: usize) -> Vec<Triple> {
        let mut found = Vec::new();
        spool
            .for_each_block(max_records, &mut |block| {
                found.extend_from_slice(block);
                Ok(())
            })
            .unwrap();
        found
    }

    #[test]
    fn file_spool_replays_exact_fixed_width_records_in_bounded_blocks() {
        let parent = test_parent("roundtrip");
        let temp = BuildTemp::new(&parent).unwrap();
        let expected = triples();
        let spool = TripleSpool::write_file(&temp, "canonical.tri", &expected).unwrap();

        assert_eq!(collect_blocks(&spool, 2), expected);
        assert_eq!(spool.count(), 4);
        drop(spool);
        drop(temp);
        std::fs::remove_dir(&parent).unwrap();
    }

    #[test]
    fn file_spool_rejects_a_truncated_canonical_record() {
        let parent = test_parent("truncated");
        let temp = BuildTemp::new(&parent).unwrap();
        let expected = triples();
        let spool = TripleSpool::write_file(&temp, "canonical.tri", &expected).unwrap();
        let path = spool.file_path().unwrap().to_path_buf();
        OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(&[0xff])
            .unwrap();

        let error = spool.for_each_block(2, &mut |_| Ok(())).unwrap_err();
        assert!(matches!(
            error,
            crate::build_pipeline::BuildPipelineError::InvalidSpool("partial triple record")
        ));
        drop(spool);
        drop(temp);
        std::fs::remove_dir(&parent).unwrap();
    }

    #[test]
    fn file_spool_keeps_its_temp_directory_alive_after_the_original_guard_drops() {
        let parent = test_parent("artifact-owner");
        let (spool, owned) = {
            let temp = BuildTemp::new(&parent).unwrap();
            let spool = TripleSpool::write_file(&temp, "canonical.tri", &triples()).unwrap();
            (spool, temp.owned_path().to_path_buf())
        };

        assert!(owned.exists());
        assert_eq!(collect_blocks(&spool, 2), triples());
        drop(spool);
        assert!(!owned.exists());
        std::fs::remove_dir(parent).unwrap();
    }

    #[test]
    fn build_temp_rejects_path_escapes() {
        let parent = test_parent("contained-path");
        let temp = BuildTemp::new(&parent).unwrap();
        assert!(temp.path("canonical.tri").is_ok());
        for escape in ["../escape", "/escape", "nested/file", ".", ""] {
            assert!(temp.path(escape).is_err(), "accepted {escape:?}");
        }
        #[cfg(windows)]
        assert!(temp.path(r"C:\\escape").is_err());
        drop(temp);
        std::fs::remove_dir(parent).unwrap();
    }

    #[test]
    fn dropping_build_temp_removes_only_its_owned_directory() {
        let parent = test_parent("cleanup");
        let sentinel = parent.join("keep-me");
        std::fs::write(&sentinel, b"sentinel").unwrap();
        let owned = {
            let temp = BuildTemp::new(&parent).unwrap();
            let owned = temp.owned_path().to_path_buf();
            assert!(owned.is_dir());
            owned
        };

        assert!(!owned.exists());
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"sentinel");
        std::fs::remove_file(sentinel).unwrap();
        std::fs::remove_dir(parent).unwrap();
    }
}
