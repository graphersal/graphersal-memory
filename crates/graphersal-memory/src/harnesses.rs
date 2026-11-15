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

/// The setup-free parts of a harness: what the daemon uses with a request's directory or a
/// harness command only.
pub struct Driver {
    /// Its sessions, as the indexing reads them.
    pub sessions: &'static dyn SessionSource,
    /// Its headless runs, when it has a command graphersal-memory can drive or refuse.
    pub headless: Option<&'static dyn crate::harness::Headless>,
}

/// The setup-free parts of `kind`.
pub fn driver(kind: Kind) -> &'static Driver {
    match kind {
        Kind::ClaudeCode => &Driver {
            sessions: &crate::sessions::ClaudeSessions,
            headless: Some(&crate::harness::ClaudeHeadless),
        },
        Kind::OpenCode => &Driver {
            sessions: &crate::opencode::sessions::Sessions,
            headless: Some(&crate::opencode::HeadlessRun),
        },
        Kind::Codex => &Driver {
            sessions: &crate::codex::sessions::Sessions,
            headless: Some(&crate::codex::HeadlessRun),
        },
        Kind::Cursor => &Driver {
            sessions: &crate::cursor::sessions::Sessions,
            headless: Some(&crate::cursor::HeadlessRun),
        },
        Kind::CodeWhale => &Driver {
            sessions: &crate::codewhale::sessions::Sessions,
            headless: Some(&crate::codewhale::HeadlessRun),
        },
    }
}

/// The sessions of `kind`, as the indexing reads them: without a setup (the daemon indexes
/// with a request's directory).
pub fn sessions(kind: Kind) -> &'static dyn SessionSource {
    driver(kind).sessions
}
