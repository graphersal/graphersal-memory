//! The registry of harnesses: the one place that names every harness graphersal-memory
//! supports (adding one: its module, its [`Kind`] variant and its lines here: its field of
//! [`Harnesses`] with its accessors, and its [`driver`]). Everything else asks the [`Harness`]
//! it gets from here.

pub mod agent;
pub mod claude_code;
pub mod codewhale;
pub mod codex;
#[cfg(test)]
pub(crate) mod conformance_tests;
pub mod cursor;
pub mod headless;
pub mod opencode;
use std::path::Path;

use crate::harnesses::agent::{Harness, Kind};
use crate::harnesses::claude_code::ClaudeCode;
use crate::harnesses::codewhale::CodeWhale;
use crate::harnesses::codex::Codex;
use crate::harnesses::cursor::Cursor;
use crate::harnesses::opencode::OpenCode;
use crate::install::Setup;
use crate::memory::sessions::SessionSource;
use crate::runtime::protocol::StartContextMode;

/// Every harness with its files on this machine: built once with the setup
/// (`Harnesses::resolve`), asked through `Setup::agent_of`.
#[derive(Debug, Clone)]
pub struct Harnesses {
    claude_code: ClaudeCode,
    opencode: OpenCode,
    codex: Codex,
    cursor: Cursor,
    codewhale: CodeWhale,
}

impl Harnesses {
    /// The harnesses with their files from the environment `env` (`probe`: opencode's line from
    /// `opencode --version`; the hooks never ask the binary); Cursor's registry of transcripts
    /// is in `data_dir`.
    pub fn resolve(
        env: &dyn Fn(&str) -> Option<String>,
        data_dir: &Path,
        probe: bool,
    ) -> Result<Self, String> {
        let detected = opencode::detect(env, None, probe)?;
        Ok(Self {
            claude_code: ClaudeCode::new(
                crate::harnesses::claude_code::install::ClaudePaths::resolve(env)?,
            ),
            opencode: OpenCode::new(opencode::OpenCodePaths::resolve_for(env, detected)?),
            codex: Codex::new(codex::CodexPaths::resolve(env)?),
            cursor: Cursor::new(cursor::CursorPaths::resolve(env, data_dir)?),
            codewhale: CodeWhale::new(codewhale::CodeWhalePaths::resolve(env)?),
        })
    }

    /// The harnesses of the home directory `home`, without environment overrides (tests).
    #[cfg(test)]
    pub fn of_home(home: &Path, data_dir: &Path) -> Self {
        let text = home.display().to_string();
        let env = |name: &str| (name == "HOME").then(|| text.clone());
        Self {
            claude_code: ClaudeCode::new(
                crate::harnesses::claude_code::install::ClaudePaths::resolve(&env)
                    .unwrap_or_else(|err| panic!("{err}")),
            ),
            opencode: OpenCode::new(opencode::OpenCodePaths::of_home(home)),
            codex: Codex::new(codex::CodexPaths::of_home(home)),
            cursor: Cursor::new(cursor::CursorPaths::of_home(home, data_dir)),
            codewhale: CodeWhale::new(codewhale::CodeWhalePaths::of_home(home)),
        }
    }

    /// The harness of `kind`.
    pub fn get(&self, kind: Kind) -> &dyn Harness {
        match kind {
            Kind::ClaudeCode => &self.claude_code,
            Kind::OpenCode => &self.opencode,
            Kind::Codex => &self.codex,
            Kind::Cursor => &self.cursor,
            Kind::CodeWhale => &self.codewhale,
        }
    }
}

/// Each harness and its files by name, for its own module (and tests): shared code asks
/// `Setup::agent_of(kind)` instead.
impl Setup {
    /// Claude Code.
    pub fn agent(&self) -> &ClaudeCode {
        &self.harnesses.claude_code
    }

    /// Claude Code's files.
    pub fn paths(&self) -> &crate::harnesses::claude_code::install::ClaudePaths {
        self.harnesses.claude_code.paths()
    }

    /// opencode's files.
    pub fn opencode(&self) -> &opencode::OpenCodePaths {
        self.harnesses.opencode.paths()
    }

    /// Codex.
    pub fn codex_agent(&self) -> &Codex {
        &self.harnesses.codex
    }

    /// Codex's files.
    pub fn codex(&self) -> &codex::CodexPaths {
        self.harnesses.codex.paths()
    }

    /// Cursor.
    pub fn cursor_agent(&self) -> &Cursor {
        &self.harnesses.cursor
    }

    /// Cursor's files (and the registry of its transcripts in the data directory).
    pub fn cursor(&self) -> &cursor::CursorPaths {
        self.harnesses.cursor.paths()
    }

    /// CodeWhale.
    pub fn codewhale_agent(&self) -> &CodeWhale {
        &self.harnesses.codewhale
    }

    /// CodeWhale's files.
    pub fn codewhale(&self) -> &codewhale::CodeWhalePaths {
        self.harnesses.codewhale.paths()
    }

    /// Replaces opencode's files (tests of another line).
    #[cfg(test)]
    pub fn set_opencode(&mut self, paths: opencode::OpenCodePaths) {
        self.harnesses.opencode = OpenCode::new(paths);
    }
}

/// The setup-free parts of a harness: what the daemon uses with a request's directory or a
/// harness command only.
pub struct Driver {
    /// Its sessions, as the indexing reads them.
    pub sessions: &'static dyn SessionSource,
    /// Its headless runs, when it has a command graphersal-memory can drive or refuse.
    pub headless: Option<&'static dyn crate::harnesses::headless::Headless>,
    /// The slugs of the memories imported from its own memory store (`NativeMemory`), which
    /// the export leaves out of the harness's files as it does for other tools' memories.
    pub native_slugs: Option<fn(&Path) -> std::collections::BTreeSet<String>>,
    /// The start context its MCP connections get, given whether its own start hook delivers
    /// it (`Harness::start_hook_installed`).
    pub start_context: fn(bool) -> StartContextMode,
}

/// The whole start context in the MCP instructions.
fn full(_hook: bool) -> StartContextMode {
    StartContextMode::Full
}

/// The setup-free parts of `kind`.
pub fn driver(kind: Kind) -> &'static Driver {
    match kind {
        Kind::ClaudeCode => &Driver {
            sessions: &crate::harnesses::claude_code::sessions::ClaudeSessions,
            headless: Some(&crate::harnesses::claude_code::ClaudeHeadless),
            native_slugs: None,
            start_context: crate::harnesses::claude_code::start_context,
        },
        Kind::OpenCode => &Driver {
            sessions: &crate::harnesses::opencode::sessions::Sessions,
            headless: Some(&crate::harnesses::opencode::HeadlessRun),
            native_slugs: None,
            start_context: full,
        },
        Kind::Codex => &Driver {
            sessions: &crate::harnesses::codex::sessions::Sessions,
            headless: Some(&crate::harnesses::codex::HeadlessRun),
            native_slugs: None,
            start_context: full,
        },
        Kind::Cursor => &Driver {
            sessions: &crate::harnesses::cursor::sessions::Sessions,
            headless: Some(&crate::harnesses::cursor::HeadlessRun),
            native_slugs: None,
            start_context: full,
        },
        Kind::CodeWhale => &Driver {
            sessions: &crate::harnesses::codewhale::sessions::Sessions,
            headless: Some(&crate::harnesses::codewhale::HeadlessRun),
            native_slugs: Some(crate::harnesses::codewhale::memory::imported_slugs),
            start_context: crate::harnesses::codewhale::start_context,
        },
    }
}

/// The sessions of `kind`, as the indexing reads them: without a setup (the daemon indexes
/// with a request's directory).
pub fn sessions(kind: Kind) -> &'static dyn SessionSource {
    driver(kind).sessions
}
