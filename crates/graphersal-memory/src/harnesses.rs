//! The registry of harnesses: the one place that names every harness graphersal-memory
//! supports (adding one: its module, its [`Kind`] variant and the line here). Everything else
//! asks the [`Harness`] it gets from here.

use crate::agents::{Harness, Kind};
use crate::install::Setup;
use crate::sessions::SessionSource;

/// The harness of `kind` with the files of `setup`.
pub fn of(setup: &Setup, kind: Kind) -> Box<dyn Harness> {
    match kind {
        Kind::ClaudeCode => Box::new(setup.agent()),
        Kind::OpenCode => Box::new(setup.opencode_agent()),
        Kind::Codex => Box::new(setup.codex_agent()),
        Kind::Cursor => Box::new(setup.cursor_agent()),
        Kind::CodeWhale => Box::new(setup.codewhale_agent()),
    }
}

/// The sessions of `kind`, as the indexing reads them: without a setup (the daemon indexes
/// with a request's directory).
pub fn sessions(kind: Kind) -> &'static dyn SessionSource {
    match kind {
        Kind::ClaudeCode => &crate::sessions::ClaudeSessions,
        Kind::OpenCode => &crate::opencode::sessions::Sessions,
        Kind::Codex => &crate::codex::sessions::Sessions,
        Kind::Cursor => &crate::cursor::sessions::Sessions,
        Kind::CodeWhale => &crate::codewhale::sessions::Sessions,
    }
}
