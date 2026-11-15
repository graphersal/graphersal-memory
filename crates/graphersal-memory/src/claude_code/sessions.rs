//! Claude Code's sessions (`crate::sessions::SessionSource`): the top-level transcripts of its
//! projects directory, read by `crate::transcript`.

use std::path::Path;

use crate::sessions::{SessionSource, Unit, scan, source_label, stamp};
use crate::transcript::{self, Transcript};

/// Claude Code's sessions: the top-level transcripts of its projects directory.
pub struct ClaudeSessions;

impl SessionSource for ClaudeSessions {
    fn units(&self, dir: &Path) -> Result<Vec<Unit>, String> {
        Ok(scan(dir)
            .into_iter()
            .filter_map(|path| {
                let (size, modified) = stamp(&path)?;
                Some(Unit {
                    path,
                    size,
                    modified,
                })
            })
            .collect())
    }

    fn read(&self, file: &Path, _data_dir: &Path) -> Result<(Transcript, String), String> {
        let bytes =
            std::fs::read(file).map_err(|err| format!("cannot read {}: {err}", file.display()))?;
        Ok((
            transcript::parse(&bytes, &self.unit_id(file)),
            source_label("transcript", file),
        ))
    }
}
