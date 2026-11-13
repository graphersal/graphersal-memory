//! The shim <-> daemon protocol (internal; it may change in any release, the protocol number
//! says which one a side speaks).
//!
//! A connection starts with ONE JSON line from the client and ONE JSON line answer:
//!
//! - a handshake: `{"shim_version", "protocol": 1, "profile": "use"|"develop", "project":
//!   {"root", "name"?}, "harness": {"name", "version"?}, "data_dir"?, "start_context"?}` ->
//!   `{"ok": true,
//!   "daemon_version", "store"}`, then the connection carries plain MCP stdio framing
//!   (newline-delimited JSON-RPC) in both directions until either side closes it;
//! - or a control request: `{"protocol": 1, "control": "status"|"stop"|"restart"}` -> one
//!   answer, then the daemon closes the connection (`stop` and `restart` then shut the daemon
//!   down; only `stop` leaves `crate::ipc::STOPPED_FILE`, so open shims do not start a new one
//!   after it). A connection closed before its first line is dropped without a log line. `{"protocol": 1, "control": "index_sessions",
//!   "projects_dir", "ended": [..], "active": [..]}` queues a session indexing run (answer `{"ok":
//!   true, "run": n}`), `{"protocol": 1, "control": "sessions_status"}` reports it (`running`,
//!   `requested`, `completed`, `report`); see `crate::sessions`. An older daemon refuses them as
//!   unknown control requests, which the callers report and survive.
//!
//!   An empty `root` is no project: the connection sees and writes the global memories only
//!   (the product's own import of another memory tool's memories, `crate::coexist`).
//!
//!   `start_context` (`full`, the default, `short` or `none`) says whether the `use` server's
//!   instructions of the connection carry the start context ([`StartContextMode`]). `{"protocol":
//!   1, "control": "start_context", "project", "max_chars"?}` answers the start context of a
//!   project root (`crate::start_context::StartContext::to_json`). `{"protocol": 1, "control":
//!   "sync_instructions", "project", "files": [{"path", "global", "loaded_by"}]}` syncs the
//!   rules of a project's instruction files for the conflict check (`crate::instructions`;
//!   answer `{"ok": true, "rules", "created", "updated", "removed", "pending", "notes",
//!   "project_id"}` right after the rules are stored; the scan for candidates runs in the
//!   background).
//!
//! - or a relayed HTTP request of the browser UI: `{"protocol": 1, "relay": "http", "port",
//!   "method", "target", "headers", "body_length", "shim_version"}` followed by the body; one
//!   answer line and its body, then the daemon closes the connection (`crate::relay`). An older
//!   daemon refuses it as a bad handshake.
//!
//! A refusal is `{"ok": false, "daemon_version", "error": {"code", "message"}}`; the codes are
//! the [`Refusal`] variants. `restarting` and `stopping` are retried by the shim (the daemon
//! exits; the next attempt starts a new one).

use serde_json::{Value, json};

use crate::cli::Profile;

/// The protocol number this binary speaks.
pub const PROTOCOL: u64 = 1;

/// The longest handshake or control line accepted (64 KiB).
pub const MAX_HANDSHAKE_BYTES: usize = 64 * 1024;

/// This binary's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Why the daemon refused a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The line is not a handshake (not JSON, a field missing or of the wrong type).
    BadHandshake,
    /// The client speaks another protocol number.
    Protocol,
    /// The profile is not `use` or `develop`.
    UnknownProfile,
    /// The client works on another data directory than the daemon serves.
    DataDir,
    /// The store of the project cannot be opened.
    Store,
    /// The daemon is older than the shim and exits now so a new one starts (retry).
    Restarting,
    /// The daemon is stopping (retry).
    Stopping,
    /// The daemon failed while handling the request (a bug; its log says where).
    Internal,
}

impl Refusal {
    /// The code on the wire.
    pub fn code(self) -> &'static str {
        match self {
            Refusal::BadHandshake => "bad_handshake",
            Refusal::Protocol => "protocol",
            Refusal::UnknownProfile => "unknown_profile",
            Refusal::DataDir => "data_dir",
            Refusal::Store => "store",
            Refusal::Restarting => "restarting",
            Refusal::Stopping => "stopping",
            Refusal::Internal => "internal",
        }
    }

    /// Whether the shim retries (the daemon goes away and the next attempt starts a new one).
    pub fn is_retryable_code(code: &str) -> bool {
        code == Refusal::Restarting.code() || code == Refusal::Stopping.code()
    }
}

/// A refusal answer line (without its line end).
pub fn refusal(kind: Refusal, message: &str) -> String {
    json!({"ok": false, "daemon_version": VERSION,
           "error": {"code": kind.code(), "message": message}})
    .to_string()
}

/// Whether the instructions of a `use` connection carry the start context
/// (`crate::start_context`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartContextMode {
    /// The whole start context (within `memory.start_context_chars`): a harness without a
    /// session-start hook of ours, or one we cannot tell.
    #[default]
    Full,
    /// Cut so that the instructions stay within Claude Code's limit for server instructions
    /// ([`CLAUDE_CODE_INSTRUCTIONS_CHARS`]): Claude Code without our SessionStart hook.
    Short,
    /// None: the harness's session-start hook delivers it (Claude Code with our hook), or the
    /// product's own connections (import).
    None,
}

/// Claude Code cuts a server's instructions after this many characters
/// (code.claude.com/docs/en/mcp: 4,096 since v2.1.296; 2,048 before).
pub const CLAUDE_CODE_INSTRUCTIONS_CHARS: usize = 4096;

impl StartContextMode {
    /// The name on the wire.
    pub fn name(self) -> &'static str {
        match self {
            StartContextMode::Full => "full",
            StartContextMode::Short => "short",
            StartContextMode::None => "none",
        }
    }

    /// The mode of a wire name (an unknown or missing one is `Full`).
    pub fn parse(name: Option<&str>) -> Self {
        match name {
            Some("short") => StartContextMode::Short,
            Some("none") => StartContextMode::None,
            _ => StartContextMode::Full,
        }
    }
}

/// A handshake as the daemon reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    /// The shim's version.
    pub shim_version: String,
    /// The connection's profile.
    pub profile: Profile,
    /// The project key (its root).
    pub project: String,
    /// The harness's name (diagnostics only).
    pub harness: String,
    /// The data directory the shim works on, when it says.
    pub data_dir: Option<String>,
    /// Whether the instructions carry the start context.
    pub start_context: StartContextMode,
}

/// What the first line of a connection asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opening {
    /// An MCP connection.
    Handshake(Handshake),
    /// A control request (`status`, `stop`).
    Control(String),
    /// A relayed HTTP request of the browser UI (`crate::relay`).
    Relay(crate::relay::Frame),
}

/// Parses the first line of a connection; the error is the refusal to answer.
pub fn parse_opening(line: &[u8]) -> Result<Opening, (Refusal, String)> {
    let bad = |what: &str| {
        (
            Refusal::BadHandshake,
            format!(
                "the first line is not a graphersal-memory handshake ({what}); expected \
                 {{\"protocol\": {PROTOCOL}, \"shim_version\", \"profile\", \"project\": \
                 {{\"root\"}}, \"harness\": {{\"name\"}}}}"
            ),
        )
    };
    let value: Value = serde_json::from_slice(line).map_err(|_| bad("not JSON"))?;
    let object = value.as_object().ok_or_else(|| bad("not an object"))?;
    let protocol = object
        .get("protocol")
        .and_then(Value::as_u64)
        .ok_or_else(|| bad("no protocol number"))?;
    if protocol != PROTOCOL {
        return Err((
            Refusal::Protocol,
            format!(
                "protocol {protocol} is not supported by this daemon (version {VERSION}, \
                 protocol {PROTOCOL}): use a graphersal-memory binary of the same release for \
                 the shim and the daemon"
            ),
        ));
    }
    if object.contains_key("relay") {
        return crate::relay::parse(object)
            .map(Opening::Relay)
            .map_err(|message| (Refusal::BadHandshake, message));
    }
    if let Some(control) = object.get("control") {
        let control = control
            .as_str()
            .ok_or_else(|| bad("control is not a string"))?;
        return Ok(Opening::Control(control.to_string()));
    }
    let text = |key: &str| object.get(key).and_then(Value::as_str);
    let shim_version = text("shim_version").ok_or_else(|| bad("no shim_version"))?;
    let profile = text("profile").ok_or_else(|| bad("no profile"))?;
    let profile = match profile {
        "use" => Profile::Use,
        "develop" => Profile::Develop,
        other => {
            return Err((
                Refusal::UnknownProfile,
                format!("unknown profile '{other}': expected use or develop"),
            ));
        }
    };
    let project = object
        .get("project")
        .and_then(|project| project.get("root"))
        .and_then(Value::as_str)
        .ok_or_else(|| bad("no project root"))?;
    let harness = object
        .get("harness")
        .and_then(|harness| harness.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(|| bad("no harness name"))?;
    Ok(Opening::Handshake(Handshake {
        shim_version: shim_version.to_string(),
        profile,
        project: project.to_string(),
        harness: harness.to_string(),
        data_dir: text("data_dir").map(str::to_string),
        start_context: StartContextMode::parse(text("start_context")),
    }))
}

/// The handshake line the shim sends (without its line end), with the whole start context.
#[cfg(test)]
pub fn handshake_line(profile: Profile, project: &str, harness: &str, data_dir: &str) -> String {
    handshake_line_with(profile, project, harness, data_dir, StartContextMode::Full)
}

/// [`handshake_line`] with the start context mode `start`.
pub fn handshake_line_with(
    profile: Profile,
    project: &str,
    harness: &str,
    data_dir: &str,
    start: StartContextMode,
) -> String {
    let name = std::path::Path::new(project)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    json!({
        "shim_version": VERSION,
        "protocol": PROTOCOL,
        "profile": match profile { Profile::Use => "use", Profile::Develop => "develop" },
        "project": {"root": project, "name": name},
        "harness": {"name": harness, "version": null},
        "data_dir": data_dir,
        "start_context": start.name(),
    })
    .to_string()
}

/// A control request line (without its line end).
pub fn control_line(control: &str) -> String {
    json!({"protocol": PROTOCOL, "control": control}).to_string()
}

/// Whether version `a` is older than version `b` (`major.minor.patch[-pre]`, semver order; a
/// version that does not parse is never older).
pub fn older(a: &str, b: &str) -> bool {
    match (parse_version(a), parse_version(b)) {
        (Some(a), Some(b)) => {
            if a.0 != b.0 {
                return a.0 < b.0;
            }
            match (a.1, b.1) {
                (Some(_), None) => true,
                (Some(x), Some(y)) => x < y,
                _ => false,
            }
        }
        _ => false,
    }
}

type Version<'a> = ((u64, u64, u64), Option<&'a str>);

fn parse_version(text: &str) -> Option<Version<'_>> {
    let text = text.split('+').next()?;
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (text, None),
    };
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let triple = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some((triple, pre))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handshake_round_trips() {
        let line = handshake_line(Profile::Develop, "/w/app", "claude-code", "/d");
        let Ok(Opening::Handshake(handshake)) = parse_opening(line.as_bytes()) else {
            panic!("{line}");
        };
        assert_eq!(handshake.profile, Profile::Develop);
        assert_eq!(handshake.project, "/w/app");
        assert_eq!(handshake.harness, "claude-code");
        assert_eq!(handshake.shim_version, VERSION);
        assert_eq!(handshake.data_dir.as_deref(), Some("/d"));
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["project"]["name"], "app");
        assert_eq!(
            parse_opening(control_line("status").as_bytes()),
            Ok(Opening::Control("status".into()))
        );
    }

    #[test]
    fn the_start_context_mode_round_trips_and_defaults_to_full() {
        for mode in [
            StartContextMode::Full,
            StartContextMode::Short,
            StartContextMode::None,
        ] {
            let line = handshake_line_with(Profile::Use, "/w", "claude-code", "/d", mode);
            let Ok(Opening::Handshake(handshake)) = parse_opening(line.as_bytes()) else {
                panic!("{line}");
            };
            assert_eq!(handshake.start_context, mode);
        }
        let old = br#"{"protocol":1,"shim_version":"0.0.0","profile":"use","project":{"root":"/w"},"harness":{"name":"x"}}"#;
        let Ok(Opening::Handshake(handshake)) = parse_opening(old) else {
            panic!("an older shim's handshake");
        };
        assert_eq!(handshake.start_context, StartContextMode::Full);
    }

    #[test]
    fn refusals_name_what_is_wrong() {
        let (kind, message) = parse_opening(b"hello").unwrap_err();
        assert_eq!(kind, Refusal::BadHandshake);
        assert!(message.contains("not JSON"), "{message}");
        let (kind, message) = parse_opening(
            br#"{"protocol":1,"shim_version":"0.0.0","profile":"admin","project":{"root":"/w"},"harness":{"name":"x"}}"#,
        )
        .unwrap_err();
        assert_eq!(kind, Refusal::UnknownProfile);
        assert!(message.contains("'admin'") && message.contains("use or develop"));
        let (kind, message) = parse_opening(br#"{"protocol":2,"control":"status"}"#).unwrap_err();
        assert_eq!(kind, Refusal::Protocol);
        assert!(message.contains("protocol 2"), "{message}");
        let (kind, message) =
            parse_opening(br#"{"protocol":1,"shim_version":"0.0.0","profile":"use"}"#).unwrap_err();
        assert_eq!(kind, Refusal::BadHandshake);
        assert!(message.contains("no project root"), "{message}");
        let line = refusal(Refusal::Stopping, "bye");
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "stopping");
        assert!(Refusal::is_retryable_code("restarting"));
        assert!(!Refusal::is_retryable_code("protocol"));
    }

    #[test]
    fn an_empty_root_is_no_project() {
        let opening = parse_opening(
            br#"{"protocol":1,"shim_version":"0.0.0","profile":"use","project":{"root":""},"harness":{"name":"graphersal-memory"}}"#,
        )
        .unwrap();
        match opening {
            Opening::Handshake(handshake) => assert_eq!(handshake.project, ""),
            _ => panic!("a handshake"),
        }
    }

    #[test]
    fn versions_compare_in_semver_order() {
        assert!(older("0.0.0-old", "0.0.0"));
        assert!(older("0.1.9", "0.2.0"));
        assert!(older("1.0.0-alpha", "1.0.0-beta"));
        assert!(!older("0.2.0", "0.2.0"));
        assert!(!older("0.3.0", "0.2.0"));
        assert!(!older("0.2.0", "0.2.0-rc1"));
        assert!(!older("garbage", "0.1.0"));
        assert!(!older("0.1.0+build", "0.1.0"));
    }
}
