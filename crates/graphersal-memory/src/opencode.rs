//! opencode (1.x; verified against v1.19.5, github.com/anomalyco/opencode): where it keeps its
//! files, what graphersal-memory writes there, and how its sessions are read.
//!
//! - **Configuration**: the global directory is `$XDG_CONFIG_HOME/opencode`, else
//!   `~/.config/opencode` (`packages/core/src/global.ts`, the `xdg-basedir` package; also on
//!   Windows, under the home directory). opencode merges `config.json`, `opencode.json` and
//!   `opencode.jsonc` from it (`config/config.ts`, `loadGlobal`), all JSONC (comments and
//!   trailing commas allowed). install edits the first that exists (`opencode.jsonc`,
//!   `opencode.json`, `config.json`), else creates `opencode.json`, as text (`crate::jsonedit`).
//! - **MCP servers**: members of `mcp`, a local stdio server being
//!   `{"type": "local", "command": [program, args...], "enabled": true}`
//!   (`packages/core/src/v1/config/mcp.ts`). The develop server is registered with
//!   `"enabled": false` until it is enabled: opencode has a switched-off state.
//! - **Rules**: opencode loads the first of `~/.config/opencode/AGENTS.md` and
//!   `~/.claude/CLAUDE.md` that exists (`session/instruction.ts`), so creating `AGENTS.md` would
//!   silently stop it from loading a Claude Code user's `CLAUDE.md`. The managed rule block
//!   therefore goes into a file of our own, [`RULES_FILE`] in the configuration directory,
//!   registered in the configuration's `instructions` list (absolute paths are loaded in every
//!   session). The user's `AGENTS.md` is never touched.
//! - **Session start and end**: a plugin file of our own, [`PLUGIN_FILE`] in `plugins/`
//!   (opencode auto-loads `{plugin,plugins}/*.{ts,js}` of the configuration directory,
//!   `config/plugin.ts`). It runs `graphersal-memory hook session-start|session-end --harness
//!   opencode` detached on `session.created` and when opencode shuts down (the `dispose` hook;
//!   opencode has no session-end event), and shows the start hook's line as a toast
//!   (`client.tui.showToast`; opencode has no status line a plugin can set).
//! - **Sessions**: a SQLite database, `$XDG_DATA_HOME/opencode/opencode.db` (default
//!   `~/.local/share/opencode`; `OPENCODE_DB` when set to an absolute path), read by
//!   [`sessions`].
//! - **Memory**: opencode has no memory feature of its own (nothing to switch off) and no file
//!   memory: the export of `uninstall` writes readable Markdown files into
//!   `~/graphersal-memory-export/opencode/` instead ([`OpenCode::memory_dir`]).

pub mod install;
pub mod sessions;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::agents::{Agent, Kind};

/// The managed rules file in opencode's configuration directory.
pub const RULES_FILE: &str = "graphersal-memory.md";

/// The plugin file in opencode's `plugins/` directory.
pub const PLUGIN_FILE: &str = "graphersal-memory.js";

/// The configuration files opencode reads from its global directory, in the order install
/// prefers them for its edit (the one opencode merges last first).
pub const CONFIG_FILES: [&str; 3] = ["opencode.jsonc", "opencode.json", "config.json"];

/// The configuration's JSON schema (what a file install creates names).
pub const SCHEMA: &str = "https://opencode.ai/config.json";

/// The default headless command of opencode's summaries and bench: a model of opencode Zen
/// that is free (for a limited time, opencode docs "Zen") and needs no login.
pub const DEFAULT_HEADLESS_COMMAND: &str = "opencode run --model opencode/big-pickle";

/// Where opencode keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodePaths {
    /// The home directory.
    pub home: PathBuf,
    /// The global configuration directory.
    pub config_dir: PathBuf,
    /// The configuration file install edits (it may not exist yet).
    pub config_file: PathBuf,
    /// The managed rules file.
    pub rules_file: PathBuf,
    /// The plugin file.
    pub plugin_file: PathBuf,
    /// The session database.
    pub db: PathBuf,
    /// Where `uninstall` exports the memories as Markdown.
    pub export_dir: PathBuf,
}

impl OpenCodePaths {
    /// Resolves the paths from the environment (`env` reads one variable): `XDG_CONFIG_HOME`,
    /// `XDG_DATA_HOME`, `OPENCODE_DB`, home `HOME` else `USERPROFILE`.
    pub fn resolve(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let set = |name: &str| env(name).filter(|value| !value.is_empty());
        let home = PathBuf::from(
            set("HOME")
                .or_else(|| set("USERPROFILE"))
                .ok_or("cannot find the home directory (HOME is not set)")?,
        );
        let config_dir = set("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("opencode");
        let data_dir = set("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local").join("share"))
            .join("opencode");
        let db = match set("OPENCODE_DB") {
            Some(db) if Path::new(&db).is_absolute() => PathBuf::from(db),
            Some(db) if db != ":memory:" => data_dir.join(db),
            _ => data_dir.join("opencode.db"),
        };
        let config_file = CONFIG_FILES
            .iter()
            .map(|name| config_dir.join(name))
            .find(|path| path.is_file())
            .unwrap_or_else(|| config_dir.join("opencode.json"));
        Ok(Self {
            rules_file: config_dir.join(RULES_FILE),
            plugin_file: config_dir.join("plugins").join(PLUGIN_FILE),
            export_dir: home.join("graphersal-memory-export").join("opencode"),
            config_file,
            config_dir,
            db,
            home,
        })
    }

    /// The paths of a home directory, without environment overrides (tests).
    #[cfg(test)]
    pub fn of_home(home: &Path) -> Self {
        let text = home.display().to_string();
        // `resolve` fails only without a home directory, which is given here.
        Self::resolve(&|name| (name == "HOME").then(|| text.clone())).unwrap_or_else(|_| Self {
            home: home.to_path_buf(),
            config_dir: home.join(".config/opencode"),
            config_file: home.join(".config/opencode/opencode.json"),
            rules_file: home.join(".config/opencode").join(RULES_FILE),
            plugin_file: home.join(".config/opencode/plugins").join(PLUGIN_FILE),
            db: home.join(".local/share/opencode/opencode.db"),
            export_dir: home.join("graphersal-memory-export/opencode"),
        })
    }
}

/// opencode, as the installation sees it.
#[derive(Debug, Clone)]
pub struct OpenCode {
    paths: OpenCodePaths,
}

impl OpenCode {
    /// opencode with its files at `paths`.
    pub fn new(paths: OpenCodePaths) -> Self {
        Self { paths }
    }
}

impl Agent for OpenCode {
    fn kind(&self) -> Kind {
        Kind::OpenCode
    }

    fn mcp_config_file(&self) -> &Path {
        &self.paths.config_file
    }

    fn mcp_servers_key(&self) -> &'static str {
        "mcp"
    }

    /// `{"type": "local", "command": [exe, args...], "enabled": true}` (opencode docs "MCP
    /// servers", `packages/core/src/v1/config/mcp.ts`).
    fn mcp_entry(&self, exe: &Path, args: &[String]) -> Value {
        let mut command = vec![exe.display().to_string()];
        command.extend(args.iter().cloned());
        json!({"type": "local", "command": command, "enabled": true})
    }

    fn remove_server_hint(&self, name: &str) -> String {
        format!(
            "the member mcp.{name} of {}",
            self.paths.config_file.display()
        )
    }

    /// The configuration file (opencode keeps its settings and MCP servers in one file).
    fn settings_file(&self) -> &Path {
        &self.paths.config_file
    }

    fn instruction_file(&self) -> &Path {
        &self.paths.rules_file
    }

    fn projects_dir(&self) -> &Path {
        &self.paths.db
    }

    fn has_file_memory(&self) -> bool {
        false
    }

    /// `~/graphersal-memory-export/opencode/<encoded root>/` (MEMORY.md and one file per
    /// memory, readable Markdown; opencode loads none of it by itself).
    fn memory_dir(&self, root: &Path) -> PathBuf {
        self.paths
            .export_dir
            .join(crate::import::claude_slug(&root.display().to_string()))
    }

    fn global_memory_file(&self) -> PathBuf {
        self.paths.export_dir.join("global.md")
    }

    /// opencode has no memory feature of its own.
    fn auto_memory_key(&self) -> Option<&'static str> {
        None
    }

    fn headless_command(&self) -> &'static str {
        DEFAULT_HEADLESS_COMMAND
    }
}

/// Whether `command` runs opencode.
pub fn is_opencode(command: &[String]) -> bool {
    command.first().is_some_and(|program| {
        Path::new(program)
            .file_stem()
            .is_some_and(|stem| stem == "opencode")
    })
}

/// The model of an opencode command (`--model`/`-m provider/model`), when it names one.
pub fn model_of(command: &[String]) -> Option<&str> {
    command
        .windows(2)
        .find(|pair| pair[0] == "--model" || pair[0] == "-m")
        .map(|pair| pair[1].as_str())
        .or_else(|| command.iter().find_map(|arg| arg.strip_prefix("--model=")))
}

/// Whether an opencode command uses a free model of opencode Zen (`opencode/big-pickle` and
/// the `opencode/*-free` models, opencode docs "Zen", "free for a limited time"): opencode
/// prices them at 0, so the summaries and the bench cost nothing.
pub fn free_model(command: &[String]) -> bool {
    is_opencode(command)
        && model_of(command).is_some_and(|model| {
            model
                .strip_prefix("opencode/")
                .is_some_and(|id| id == "big-pickle" || id.ends_with("-free"))
        })
}

/// The environment of a headless opencode run in a sandbox directory: its own configuration
/// and data directories (so the user's global configuration, plugins, rules and session
/// database are neither loaded nor written), no project configuration, nothing read from
/// Claude Code's files, no update check, and graphersal-memory's hooks off.
pub fn sandbox_env(config_home: &Path, data_home: &Path) -> Vec<(String, String)> {
    vec![
        ("XDG_CONFIG_HOME".into(), config_home.display().to_string()),
        ("XDG_DATA_HOME".into(), data_home.display().to_string()),
        ("OPENCODE_DISABLE_PROJECT_CONFIG".into(), "1".into()),
        ("OPENCODE_DISABLE_CLAUDE_CODE".into(), "1".into()),
        ("OPENCODE_DISABLE_AUTOUPDATE".into(), "1".into()),
        (crate::summaries::HOOK_ENV.into(), "off".into()),
    ]
}

/// Builds a sandbox for headless opencode runs in `dir`: `config/opencode/opencode.json` with
/// `config` (the arm's MCP servers, permissions and instructions), and `data/opencode/` with a
/// copy of the user's `auth.json` (their provider logins; read, never changed) when there is
/// one. Returns the environment of the runs.
pub fn build_sandbox(
    dir: &Path,
    config: &Value,
    user_db: Option<&Path>,
) -> Result<Vec<(String, String)>, String> {
    let config_home = dir.join("config");
    let data_home = dir.join("data");
    let config_dir = config_home.join("opencode");
    let data_dir = data_home.join("opencode");
    for d in [&config_dir, &data_dir] {
        std::fs::create_dir_all(d)
            .map_err(|err| format!("cannot create {}: {err}", d.display()))?;
    }
    let mut config = config.clone();
    config["$schema"] = json!(SCHEMA);
    crate::fsutil::write_atomic(
        &config_dir.join("opencode.json"),
        format!("{}\n", crate::jsonedit::render(&config, Some("  "), 0)).as_bytes(),
    )?;
    if let Some(auth) = user_db.and_then(Path::parent).map(|d| d.join("auth.json"))
        && auth.is_file()
    {
        let target = data_dir.join("auth.json");
        std::fs::copy(&auth, &target)
            .map_err(|err| format!("cannot copy {}: {err}", auth.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600));
        }
    }
    Ok(sandbox_env(&config_home, &data_home))
}

/// The permissions of a sandboxed run: everything denied, then `allow` (tool names or
/// patterns) allowed (opencode docs "Permissions": the last matching rule wins).
pub fn permissions(allow: &[&str]) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("*".into(), json!("deny"));
    for tool in allow {
        map.insert((*tool).to_string(), json!("allow"));
    }
    Value::Object(map)
}

/// `opencode run --format json`'s event lines turned into the object `claude -p
/// --output-format json` answers (what the summaries and the bench read): `result` = the text
/// parts, `total_cost_usd` and `usage` = the sums of the `step_finish` parts, `num_turns` =
/// the steps, `is_error` for an `error` event or no text, `structured_output` = the JSON
/// object in the text when there is one.
pub fn normalize_run_output(stdout: &str) -> Value {
    let mut text = Vec::new();
    let (mut cost, mut input, mut output, mut steps) = (0.0, 0u64, 0u64, 0u64);
    let mut error: Option<String> = None;
    let mut events = 0;
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        events += 1;
        let part = &event["part"];
        match event["type"].as_str().unwrap_or_default() {
            "text" => {
                if let Some(t) = part["text"].as_str()
                    && part["synthetic"] != true
                {
                    text.push(t.to_string());
                }
            }
            "step_finish" => {
                steps += 1;
                cost += part["cost"].as_f64().unwrap_or(0.0);
                let tokens = &part["tokens"];
                let n = |v: &Value| v.as_u64().unwrap_or(0);
                input += n(&tokens["input"])
                    + n(&tokens["cache"]["read"])
                    + n(&tokens["cache"]["write"]);
                output += n(&tokens["output"]) + n(&tokens["reasoning"]);
            }
            "error" => {
                let e = &event["error"];
                error = Some(
                    e["data"]["message"]
                        .as_str()
                        .or_else(|| e["message"].as_str())
                        .or_else(|| e["name"].as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| e.to_string()),
                );
            }
            _ => {}
        }
    }
    let result = text.join("\n\n");
    let mut value = json!({
        "type": "result", "total_cost_usd": cost, "num_turns": steps,
        "usage": {"input_tokens": input, "output_tokens": output},
    });
    if let Some(error) = error {
        value["is_error"] = json!(true);
        value["result"] = json!(error);
    } else if result.trim().is_empty() {
        value["is_error"] = json!(true);
        value["result"] = json!(if events == 0 {
            "opencode answered no JSON events".to_string()
        } else {
            "opencode answered no text".to_string()
        });
    } else {
        value["is_error"] = json!(false);
        if let Some(object) = crate::bench::json_in(&result) {
            value["structured_output"] = object;
        }
        value["result"] = json!(result);
    }
    value
}

/// The plugin file's text: for `exe` with `dir_args` (the data and run directory flags, when
/// they were chosen explicitly). It only starts our hook and never waits for it, so a session
/// is never blocked; `GRAPHERSAL_MEMORY_HOOK=off` (the summaries' and the bench's own runs)
/// makes it do nothing.
pub fn plugin_text(exe: &Path, dir_args: &[String]) -> String {
    let mut command = vec![exe.display().to_string()];
    command.extend(dir_args.iter().cloned());
    let command = Value::from(command).to_string();
    format!(
        "// {MARKER}\n\
// Written by `graphersal-memory install`; `graphersal-memory uninstall` removes it. Edits are\n\
// kept: install then reports a conflict instead of overwriting the file.\n\
// opencode loads every file of this directory at startup. On session.created it runs\n\
// `graphersal-memory hook session-start --harness opencode` (queues the indexing of past\n\
// sessions, shows one line as a toast); when opencode shuts down it runs `session-end` for the\n\
// sessions it saw. Both are detached: a session never waits for them.\n\
const COMMAND = {command};\n\
\n\
export const GraphersalMemory = async ({{ client }}) => {{\n\
  const off = () => (process.env.GRAPHERSAL_MEMORY_HOOK ?? \"\") === \"off\";\n\
  const seen = new Map();\n\
  const run = (event, input, show) => {{\n\
    if (off()) return;\n\
    try {{\n\
      const child = Bun.spawn([COMMAND[0], \"hook\", event, \"--harness\", \"opencode\", ...COMMAND.slice(1)], {{\n\
        stdin: new Blob([JSON.stringify(input)]),\n\
        stdout: show ? \"pipe\" : \"ignore\",\n\
        stderr: \"ignore\",\n\
      }});\n\
      if (show) {{\n\
        new Response(child.stdout).text().then((line) => {{\n\
          const message = line.trim();\n\
          if (message) client.tui.showToast({{ body: {{ message, variant: \"info\" }} }}).catch(() => {{}});\n\
        }}).catch(() => {{}});\n\
      }} else {{\n\
        child.unref();\n\
      }}\n\
    }} catch {{\n\
      // The memory never breaks a session.\n\
    }}\n\
  }};\n\
  return {{\n\
    event: async ({{ event }}) => {{\n\
      if (event.type !== \"session.created\") return;\n\
      const info = event.properties?.info ?? {{}};\n\
      if (!info.id || info.parentID) return;\n\
      seen.set(info.id, info.directory ?? \"\");\n\
      run(\"session-start\", {{ session_id: info.id, cwd: info.directory ?? \"\" }}, true);\n\
    }},\n\
    dispose: async () => {{\n\
      for (const [id, cwd] of seen) run(\"session-end\", {{ session_id: id, cwd }}, false);\n\
    }},\n\
  }};\n\
}};\n"
    )
}

/// The first line of our plugin file (how it is recognised).
pub const MARKER: &str = "graphersal-memory plugin (managed by `graphersal-memory install`)";

/// Whether a file's text is our plugin.
pub fn is_our_plugin(text: &str) -> bool {
    text.lines()
        .next()
        .is_some_and(|line| line.trim_start_matches("// ") == MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_follow_xdg_then_home() {
        let paths = OpenCodePaths::resolve(&|name| match name {
            "HOME" => Some("/h".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(paths.config_dir, PathBuf::from("/h/.config/opencode"));
        assert_eq!(
            paths.config_file,
            PathBuf::from("/h/.config/opencode/opencode.json")
        );
        assert_eq!(
            paths.plugin_file,
            PathBuf::from("/h/.config/opencode/plugins/graphersal-memory.js")
        );
        assert_eq!(
            paths.db,
            PathBuf::from("/h/.local/share/opencode/opencode.db")
        );
        let custom = OpenCodePaths::resolve(&|name| match name {
            "HOME" => Some("/h".into()),
            "XDG_CONFIG_HOME" => Some("/c".into()),
            "XDG_DATA_HOME" => Some("/d".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            custom.rules_file,
            PathBuf::from("/c/opencode/graphersal-memory.md")
        );
        assert_eq!(custom.db, PathBuf::from("/d/opencode/opencode.db"));
        let db = OpenCodePaths::resolve(&|name| match name {
            "HOME" => Some("/h".into()),
            "OPENCODE_DB" => Some("/x/my.db".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(db.db, PathBuf::from("/x/my.db"));
        assert!(OpenCodePaths::resolve(&|_| None).is_err());
    }

    #[test]
    fn an_existing_jsonc_file_is_the_one_edited() {
        let home = crate::mcp_tests::TempDir::new("oc-paths");
        let dir = home.path().join(".config/opencode");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), "{}").unwrap();
        assert_eq!(
            OpenCodePaths::of_home(home.path()).config_file,
            dir.join("config.json")
        );
        std::fs::write(dir.join("opencode.jsonc"), "{}").unwrap();
        assert_eq!(
            OpenCodePaths::of_home(home.path()).config_file,
            dir.join("opencode.jsonc")
        );
    }

    #[test]
    fn entries_free_models_and_the_plugin_are_recognised() {
        let agent = OpenCode::new(OpenCodePaths::of_home(Path::new("/h")));
        let entry = agent.mcp_entry(Path::new("/x/graphersal-memory"), &["mcp".into()]);
        assert_eq!(entry["command"], json!(["/x/graphersal-memory", "mcp"]));
        assert!(crate::agents::is_our_entry(&entry));
        let words = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert!(free_model(&words(
            "opencode run --model opencode/big-pickle"
        )));
        assert!(free_model(&words("/bin/opencode run -m opencode/exo-free")));
        assert!(!free_model(&words(
            "opencode run --model anthropic/claude-sonnet-4"
        )));
        assert!(!free_model(&words("claude -p --model opencode/big-pickle")));
        let plugin = plugin_text(
            Path::new("/x/graphersal-memory"),
            &["--data-dir".into(), "/d".into()],
        );
        assert!(is_our_plugin(&plugin));
        assert!(
            plugin.contains("const COMMAND = [\"/x/graphersal-memory\",\"--data-dir\",\"/d\"];")
        );
        assert!(!is_our_plugin("export const X = 1\n"));
    }

    #[test]
    fn run_events_become_the_answer_object() {
        let stdout = [
            json!({"type": "step_start", "part": {"type": "step-start"}}),
            json!({"type": "text", "part": {"type": "text", "text": "{\"headline\": \"h\", \"summary\": \"s\"}"}}),
            json!({"type": "step_finish", "part": {"type": "step-finish", "cost": 0.25,
                "tokens": {"input": 100, "output": 20, "reasoning": 5, "cache": {"read": 7, "write": 3}}}}),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let value = normalize_run_output(&stdout);
        assert_eq!(value["is_error"], false);
        assert_eq!(value["total_cost_usd"], 0.25);
        assert_eq!(value["usage"]["input_tokens"], 110);
        assert_eq!(value["usage"]["output_tokens"], 25);
        assert_eq!(value["num_turns"], 1);
        assert_eq!(value["structured_output"]["headline"], "h");
        let failed = normalize_run_output(
            &json!({"type": "error", "error": {"name": "APIError", "data": {"message": "no key"}}})
                .to_string(),
        );
        assert_eq!(
            (failed["is_error"].clone(), failed["result"].clone()),
            (json!(true), json!("no key"))
        );
        assert_eq!(normalize_run_output("not json")["is_error"], true);
    }
}
