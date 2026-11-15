//! The registry of harnesses: the one place that names every harness graphersal-memory
//! supports (adding one: its module, its [`Kind`] variant and the line here). Everything else
//! asks the [`Harness`] it gets from here.

pub mod agent;
pub mod claude_code;
pub mod codewhale;
pub mod codex;
#[cfg(test)]
pub(crate) mod conformance_tests;
pub mod cursor;
pub mod headless;
pub mod opencode;
use crate::harnesses::agent::{Harness, Kind};
use crate::install::Setup;
use crate::memory::sessions::SessionSource;

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
    pub headless: Option<&'static dyn crate::harnesses::headless::Headless>,
}

/// The setup-free parts of `kind`.
pub fn driver(kind: Kind) -> &'static Driver {
    match kind {
        Kind::ClaudeCode => &Driver {
            sessions: &crate::harnesses::claude_code::sessions::ClaudeSessions,
            headless: Some(&crate::harnesses::claude_code::ClaudeHeadless),
        },
        Kind::OpenCode => &Driver {
            sessions: &crate::harnesses::opencode::sessions::Sessions,
            headless: Some(&crate::harnesses::opencode::HeadlessRun),
        },
        Kind::Codex => &Driver {
            sessions: &crate::harnesses::codex::sessions::Sessions,
            headless: Some(&crate::harnesses::codex::HeadlessRun),
        },
        Kind::Cursor => &Driver {
            sessions: &crate::harnesses::cursor::sessions::Sessions,
            headless: Some(&crate::harnesses::cursor::HeadlessRun),
        },
        Kind::CodeWhale => &Driver {
            sessions: &crate::harnesses::codewhale::sessions::Sessions,
            headless: Some(&crate::harnesses::codewhale::HeadlessRun),
        },
    }
}

/// The sessions of `kind`, as the indexing reads them: without a setup (the daemon indexes
/// with a request's directory).
pub fn sessions(kind: Kind) -> &'static dyn SessionSource {
    driver(kind).sessions
}
