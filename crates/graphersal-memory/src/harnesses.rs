//! The registry of harnesses: the one place that names every harness graphersal-memory
//! supports (adding one: its module, its [`Kind`] variant and the line here). Everything else
//! asks the [`Harness`] it gets from here.

use crate::agents::{Harness, Kind};
use crate::install::Setup;

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
