//! Tests of `crate::coexist` (M15): other memory beside graphersal-memory, on temporary homes
//! with realistic configurations of other tools (never the user's), the memory served in the
//! test process as in `crate::install_tests`.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::agents::{Agent, Kind};
use crate::coexist::{self, ServerState};
use crate::import::{self, ImportState};
use crate::install::{self, InstallOptions, Setup};
use crate::install_tests::{
    Memory, bare_setup_in, existing_home, memory_fixture, read, run_install, run_uninstall,
    run_wizard, setup_in, snapshot, uninstall_yes, with_programs, yes,
};
use crate::mcp;
use crate::mcp_tests::TempDir;

/// The reference memory server's entry in Claude Code's user configuration, its data file at
/// `kg/memory.jsonl` of the home.
fn reference_entry(home: &Path) -> Value {
    json!({"type": "stdio", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-memory"],
           "env": {"MEMORY_FILE_PATH": home.join("kg").join("memory.jsonl").display().to_string()}})
}

/// The machine of the report: Claude Code's auto-memory on (no switch in its settings), the
/// reference memory server registered at user scope with two entities, an unrelated server, and
/// an instruction file that says to keep memory elsewhere.
pub(crate) fn named_machine(home: &Path) {
    let config = json!({"numStartups": 3, "mcpServers": {
        "memory": reference_entry(home),
        "github": {"type": "stdio", "command": "github-mcp", "args": []}}});
    std::fs::write(
        home.join(".claude.json"),
        format!("{}\n", serde_json::to_string_pretty(&config).unwrap()),
    )
    .unwrap();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(
        home.join(".claude").join("settings.json"),
        "{\n  \"model\": \"opus\"\n}\n",
    )
    .unwrap();
    std::fs::write(
        home.join(".claude").join("CLAUDE.md"),
        "# My rules\n\nBe brief.\n- Keep all memory in the knowledge graph of the memory MCP \
         server (create_entities).\n- Keep memory usage low in the hot loop.\n",
    )
    .unwrap();
    std::fs::create_dir_all(home.join("kg")).unwrap();
    std::fs::write(
        home.join("kg").join("memory.jsonl"),
        concat!(
            r#"{"type":"entity","name":"Robin","entityType":"person","observations":["Prefers short answers","Works on graphersal"]}"#,
            "\n",
            r#"{"type":"entity","name":"graphersal","entityType":"project","observations":["A graph engine in Rust"]}"#,
            "\n",
            r#"{"type":"relation","from":"Robin","to":"graphersal","relationType":"works on"}"#,
            "\n"
        ),
    )
    .unwrap();
}

fn config_json(setup: &Setup) -> Value {
    serde_json::from_str(&read(&setup.paths.user_config)).unwrap()
}

fn doctor_texts(setup: &Setup) -> Vec<String> {
    coexist::doctor_findings(setup, &coexist::detect(setup, None))
        .into_iter()
        .map(|f| match f.fix {
            Some(fix) => format!("{}\n  {fix}", f.text),
            None => f.text,
        })
        .collect()
}

#[test]
fn the_named_machine_is_detected_with_every_kind_of_finding() {
    let home = TempDir::new("co-det-home");
    let data = TempDir::new("co-det-data");
    let work = TempDir::new("co-det-work");
    let project = mcp::canonical(work.path());
    named_machine(home.path());
    memory_fixture(home.path(), &project);
    let setup = setup_in(home.path(), data.path());
    let found = coexist::detect(&setup, None);
    // The reference server is known; the unrelated github server is not memory.
    assert_eq!(found.servers.len(), 1, "{:?}", found.servers);
    let server = &found.servers[0];
    assert_eq!(server.name, "memory");
    assert_eq!(server.harness, Kind::ClaudeCode);
    assert_eq!(server.tool.map(|t| t.id), Some("server-memory"));
    assert_eq!(server.state, ServerState::On);
    assert!(server.main_by_default());
    // Claude Code's auto-memory and its files.
    assert_eq!(found.built_in.len(), 1, "{:?}", found.built_in);
    assert!(
        found.built_in[0]
            .text
            .starts_with("Claude Code's auto-memory: on; 3 memory files in 1 project"),
        "{:?}",
        found.built_in
    );
    // The instruction line; the RAM line is not about the agent's memory.
    assert_eq!(found.mentions.len(), 1, "{:?}", found.mentions);
    assert_eq!(found.mentions[0].line, 4);
    assert!(found.mentions[0].text.starts_with("- Keep all memory in"));
    assert_eq!(found.mentions[0].loaded_by, vec![Kind::ClaudeCode]);
    assert!(found.anything());
    // Its data is importable.
    let (sources, notes) = coexist::other_import::sources(&setup, &found);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].items.len(), 2);
}

#[test]
fn servers_are_found_in_every_harness_and_unknown_ones_are_kept_by_default() {
    let home = TempDir::new("co-all-home");
    let data = TempDir::new("co-all-data");
    let h = home.path();
    // Claude Code: one by its tools in the permission rules, one by its name, a local scope.
    std::fs::write(
        h.join(".claude.json"),
        r#"{"mcpServers": {"kb": {"command": "kb-server"}, "team-memory": {"url": "http://x"},
            "github": {"command": "gh"}},
            "projects": {"/w/app": {"mcpServers": {"mem0": {"command": "uvx", "args": ["mem0-mcp"]}}}}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(h.join(".claude")).unwrap();
    std::fs::write(
        h.join(".claude/settings.json"),
        r#"{"permissions": {"allow": ["mcp__kb__remember", "mcp__kb__search_code", "Bash(ls)"]},
            "enabledPlugins": {"claude-mem@thedotmack": true, "formatter@x": true}}"#,
    )
    .unwrap();
    // opencode 1.x: Basic Memory, switched off by the user, and a plugin.
    std::fs::create_dir_all(h.join(".config/opencode")).unwrap();
    std::fs::write(
        h.join(".config/opencode/opencode.jsonc"),
        "{\n  // mine\n  \"mcp\": {\"basic\": {\"type\": \"local\", \"command\": [\"uvx\", \"basic-memory\", \"mcp\"], \"enabled\": false}},\n  \"plugin\": [\"opencode-supermemory\"]\n}\n",
    )
    .unwrap();
    // Codex: Serena (a toolkit: kept by default).
    std::fs::create_dir_all(h.join(".codex")).unwrap();
    std::fs::write(
        h.join(".codex/config.toml"),
        "[mcp_servers.serena]\ncommand = \"uvx\"\nargs = [\"serena\"]\n\n[mcp_servers.fs]\ncommand = \"fs\"\n",
    )
    .unwrap();
    // Cursor: cipher.
    std::fs::create_dir_all(h.join(".cursor")).unwrap();
    std::fs::write(
        h.join(".cursor/mcp.json"),
        r#"{"mcpServers": {"cipher": {"command": "cipher", "args": ["--mode", "mcp"]}}}"#,
    )
    .unwrap();
    let setup = with_programs(
        bare_setup_in(h, data.path()),
        &["claude", "opencode", "codex", "cursor"],
    );
    let found = coexist::detect(&setup, None);
    let by_name = |name: &str| found.servers.iter().find(|s| s.name == name);
    let kb = by_name("kb").expect("kb by its tools");
    assert!(kb.tool.is_none() && kb.why.contains("remember"), "{kb:?}");
    assert!(!kb.why.contains("search_code"), "{kb:?}");
    assert!(!kb.main_by_default());
    let team = by_name("team-memory").expect("by its name");
    assert_eq!(team.why, "its name says memory");
    assert!(by_name("github").is_none() && by_name("fs").is_none());
    let mem0 = by_name("mem0").expect("local scope");
    assert_eq!(mem0.project.as_deref(), Some("/w/app"));
    let basic = by_name("basic").expect("opencode");
    assert_eq!(basic.state, ServerState::Off);
    assert_eq!(basic.tool.map(|t| t.id), Some("basic-memory"));
    let serena = by_name("serena").expect("codex");
    assert!(!serena.main_by_default());
    let cipher = by_name("cipher").expect("cursor");
    assert_eq!(cipher.tool.map(|t| t.id), Some("byterover"));
    assert!(cipher.main_by_default());
    let plugins: Vec<&str> = found.plugins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        plugins,
        ["claude-mem@thedotmack", "opencode-supermemory"],
        "{plugins:?}"
    );
    // The defaults: known memory tools named read-only, the rest kept, nothing switched off.
    let memory = Memory::open(data.path());
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            harnesses: Some(Kind::ALL.to_vec()),
            dry_run: true,
            ..yes()
        },
    );
    assert_eq!(code, 0, "{out}");
    for line in [
        "cipher (Cursor, ~/.cursor/mcp.json): ByteRover / Cipher; stays, named read-only",
        "kb (Claude Code, ~/.claude.json): a memory tool? (its tools: remember); kept as it is",
        "serena (Codex, ~/.codex/config.toml): Serena (a coding toolkit with memories); kept as \
         it is",
        "basic (opencode, ~/.config/opencode/opencode.jsonc): switched off by you; left as it is",
        "mem0 (Claude Code, ~/.claude.json, project /w/app): a project's own registration, left \
         as it is",
        "plugin or hook claude-mem@thedotmack",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    assert!(!out.contains("switch it off (the file"), "{out}");
    memory.close();
}

#[test]
fn a_default_install_names_the_reference_server_read_only_and_leaves_its_entry() {
    let home = TempDir::new("co-def-home");
    let data = TempDir::new("co-def-data");
    named_machine(home.path());
    let before_config = read(&home.path().join(".claude.json"));
    let before_rules = read(&home.path().join(".claude/CLAUDE.md"));
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    // --yes alone never switches another tool off.
    let config = config_json(&setup);
    assert_eq!(config["mcpServers"]["memory"], reference_entry(home.path()));
    let rules = read(&setup.paths.instructions);
    assert!(
        rules.starts_with(&before_rules),
        "the user's text stays: {rules}"
    );
    assert!(
        rules.contains(
            "- Other memory on this machine: the MCP server `memory`. graphersal-memory is the \
             main memory: search it only when you need to, never write there; save with \
             `remember` here.\n<!-- graphersal-memory:end -->"
        ),
        "{rules}"
    );
    // The block is ours and current: doctor and status see it installed.
    assert_eq!(
        install::inspect(&setup).block,
        install::PartState::Installed
    );
    let choices = coexist::Choices::load(&setup).unwrap();
    assert_eq!(choices.read_only, ["memory"]);
    // Its graph was imported as global memories.
    assert!(
        out.contains(
            "other memory: imported 2 memories (2 new, 0 updated; 0 unchanged) as global \
             memories from memory"
        ),
        "{out}"
    );
    let found = memory.call("", "memory", json!({"slug": "kg-robin"}));
    assert_eq!(found["scope"], "global", "{found}");
    assert!(
        found["body"]
            .as_str()
            .unwrap()
            .contains("- Robin works on graphersal"),
        "{found}"
    );
    assert_eq!(found["kind"], "reference");
    // A second install changes nothing and imports nothing again.
    let snap = snapshot(home.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), snap, "{out}");
    assert!(!out.contains("other memory: imported"), "{out}");
    // Uninstall gives everything back.
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(read(&home.path().join(".claude.json")), before_config);
    assert_eq!(read(&setup.paths.instructions), before_rules);
    assert!(coexist::Choices::load(&setup).is_none());
    memory.close();
}

#[test]
fn disable_other_switches_the_entry_off_and_uninstall_puts_it_back_byte_for_byte() {
    let home = TempDir::new("co-off-home");
    let data = TempDir::new("co-off-data");
    named_machine(home.path());
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let options = InstallOptions {
        disable_other: vec!["memory".to_string()],
        ..yes()
    };
    let (code, out) = run_install(&setup, &memory, options);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("memory: switched off in"), "{out}");
    let config = config_json(&setup);
    assert!(config["mcpServers"]["memory"].is_null(), "{config}");
    assert_eq!(config["mcpServers"]["github"]["command"], "github-mcp");
    // Not named read-only (it is off), recorded for uninstall, shown by status and doctor.
    assert!(!read(&setup.paths.instructions).contains("Other memory on this machine"));
    let state: Value =
        serde_json::from_str(&read(&data.path().join("install").join("claude-code.json"))).unwrap();
    let record = &state["files"][setup.paths.user_config.display().to_string()]
        [coexist::OFF_RECORD]["memory"];
    assert_eq!(record["how"], "removed");
    assert_eq!(record["entry"], reference_entry(home.path()));
    let status = crate::commands::status_text(data.path(), &setup.run_dir, Ok(&setup)).unwrap();
    assert!(
        status.contains(
            "memory (Claude Code, ~/.claude.json): the reference memory server \
             (@modelcontextprotocol/server-memory); switched off by graphersal-memory install"
        ),
        "{status}"
    );
    assert!(!status.contains("ACTION NEEDED"), "{status}");
    // A second install keeps it off (the current answer), changing nothing.
    let snap = snapshot(home.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), snap, "{out}");
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("memory: switched on again in"),
        "uninstall says so: {out}"
    );
    assert_eq!(snapshot(home.path()), before, "{out}");
    memory.close();
}

#[test]
fn a_config_changed_after_install_still_gets_the_entry_back() {
    let home = TempDir::new("co-chg-home");
    let data = TempDir::new("co-chg-data");
    named_machine(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let options = InstallOptions {
        disable_other: vec!["memory".to_string()],
        ..yes()
    };
    let (code, out) = run_install(&setup, &memory, options);
    assert_eq!(code, 0, "{out}");
    // Claude Code rewrites its file meanwhile (as it does with ~/.claude.json).
    let text = read(&setup.paths.user_config).replace("\"numStartups\": 3", "\"numStartups\": 4");
    std::fs::write(&setup.paths.user_config, &text).unwrap();
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    let config = config_json(&setup);
    assert_eq!(config["mcpServers"]["memory"], reference_entry(home.path()));
    assert_eq!(config["numStartups"], 4, "the newer value stays");
    assert!(config["mcpServers"]["graphersal-memory"].is_null());
    assert!(config["mcpServers"]["graphersal-memory-develop"].is_null());
    memory.close();
}

#[test]
fn entries_of_opencode_codex_and_cursor_switch_off_and_back() {
    let home = TempDir::new("co-4-home");
    let data = TempDir::new("co-4-data");
    let h = home.path();
    existing_home(h);
    std::fs::create_dir_all(h.join(".config/opencode")).unwrap();
    std::fs::write(
        h.join(".config/opencode/opencode.jsonc"),
        "{\n  // my config\n  \"mcp\": {\n    \"mem0\": {\"type\": \"local\", \"command\": [\"uvx\", \"mem0-mcp\"]}\n  }\n}\n",
    )
    .unwrap();
    std::fs::create_dir_all(h.join(".codex")).unwrap();
    std::fs::write(
        h.join(".codex/config.toml"),
        "model = \"o3\"\n\n[mcp_servers.graphiti]\ncommand = \"graphiti-mcp\" # mine\n",
    )
    .unwrap();
    std::fs::create_dir_all(h.join(".cursor")).unwrap();
    std::fs::write(
        h.join(".cursor/mcp.json"),
        "{\n  \"mcpServers\": {\n    \"supermemory\": {\"url\": \"https://mcp.supermemory.ai/sse\"}\n  }\n}\n",
    )
    .unwrap();
    let before = snapshot(h);
    let setup = with_programs(
        bare_setup_in(h, data.path()),
        &["claude", "opencode", "codex", "cursor"],
    );
    let memory = Memory::open(data.path());
    let options = InstallOptions {
        harnesses: Some(Kind::ALL.to_vec()),
        disable_other: vec![
            "mem0".to_string(),
            "graphiti".to_string(),
            "supermemory".to_string(),
        ],
        ..yes()
    };
    let (code, out) = run_install(&setup, &memory, options);
    assert_eq!(code, 0, "{out}");
    let opencode = read(&h.join(".config/opencode/opencode.jsonc"));
    let value = crate::jsonedit::value_of(&opencode).unwrap();
    assert_eq!(value["mcp"]["mem0"]["enabled"], false, "{opencode}");
    assert!(opencode.contains("// my config"));
    let codex = read(&h.join(".codex/config.toml"));
    let doc = crate::tomledit::parse(&codex).unwrap();
    assert_eq!(doc["mcp_servers"]["graphiti"]["enabled"], false, "{codex}");
    let cursor: Value = serde_json::from_str(&read(&h.join(".cursor/mcp.json"))).unwrap();
    assert!(cursor["mcpServers"]["supermemory"].is_null(), "{cursor}");
    let found = coexist::detect(&setup, None);
    assert_eq!(
        found
            .servers
            .iter()
            .filter(|s| s.state == ServerState::OffByUs)
            .count(),
        3,
        "{:?}",
        found.servers
    );
    let options = install::UninstallOptions {
        harnesses: Some(Kind::ALL.to_vec()),
        ..uninstall_yes()
    };
    let (code, out) = run_uninstall(&setup, options);
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(h), before, "{out}");
    memory.close();
}

#[test]
fn enable_other_switches_it_on_again_without_uninstall() {
    let home = TempDir::new("co-on-home");
    let data = TempDir::new("co-on-data");
    named_machine(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            disable_other: vec!["memory".to_string()],
            ..yes()
        },
    );
    assert_eq!(code, 0, "{out}");
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            enable_other: vec!["memory".to_string()],
            ..yes()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("memory: switched on again in"), "{out}");
    assert_eq!(
        config_json(&setup)["mcpServers"]["memory"],
        reference_entry(home.path())
    );
    let state = read(&data.path().join("install").join("claude-code.json"));
    assert!(!state.contains(coexist::OFF_RECORD), "{state}");
    // On again, it is named read-only (the default for a known tool).
    assert!(read(&setup.paths.instructions).contains("the MCP server `memory`"));
    // An unknown name is a warning, nothing else.
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            disable_other: vec!["nope".to_string()],
            dry_run: true,
            ..yes()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(
            "warning: --disable-other nope: no other memory tool's MCP entry of that name"
        ),
        "{out}"
    );
    memory.close();
}

#[cfg(unix)]
#[test]
fn a_symlinked_config_stays_a_link_and_a_read_only_one_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let home = TempDir::new("co-ln-home");
    let data = TempDir::new("co-ln-data");
    let dots = TempDir::new("co-ln-dots");
    named_machine(home.path());
    let target = dots.path().join("claude.json");
    std::fs::rename(home.path().join(".claude.json"), &target).unwrap();
    std::os::unix::fs::symlink(&target, home.path().join(".claude.json")).unwrap();
    let original = read(&target);
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let options = InstallOptions {
        disable_other: vec!["memory".to_string()],
        ..yes()
    };
    let (code, out) = run_install(&setup, &memory, options.clone());
    assert_eq!(code, 0, "{out}");
    let is_link = || {
        std::fs::symlink_metadata(home.path().join(".claude.json"))
            .unwrap()
            .is_symlink()
    };
    assert!(is_link());
    assert!(
        serde_json::from_str::<Value>(&read(&target)).unwrap()["mcpServers"]["memory"].is_null()
    );
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert!(is_link());
    assert_eq!(read(&target), original);
    // A read-only configuration: refused, nothing changes.
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o444)).unwrap();
    let before = snapshot(home.path());
    let mut connect = memory.connector();
    let result = install::install(
        &setup,
        &options,
        &mut connect,
        &mut &b""[..],
        &mut Vec::new(),
    );
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
    let err = result.unwrap_err();
    assert!(err.contains("nothing was changed"), "{err}");
    assert_eq!(snapshot(home.path()), before);
    assert_eq!(read(&target), original);
    memory.close();
}

#[cfg(unix)]
#[test]
fn a_write_that_fails_midway_puts_the_other_entry_back() {
    use std::os::unix::fs::PermissionsExt;
    let home = TempDir::new("co-b6-home");
    let data = TempDir::new("co-b6-data");
    named_machine(home.path());
    std::fs::create_dir_all(home.path().join(".codex")).unwrap();
    std::fs::write(home.path().join(".codex/config.toml"), "model = \"o3\"\n").unwrap();
    let before = snapshot(home.path());
    let setup = with_programs(
        bare_setup_in(home.path(), data.path()),
        &["claude", "codex"],
    );
    let memory = Memory::open(data.path());
    let codex = home.path().join(".codex");
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o555)).unwrap();
    let mut connect = memory.connector();
    let options = InstallOptions {
        harnesses: Some(vec![Kind::ClaudeCode, Kind::Codex]),
        disable_other: vec!["memory".to_string()],
        ..yes()
    };
    let result = install::install(
        &setup,
        &options,
        &mut connect,
        &mut &b""[..],
        &mut Vec::new(),
    );
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = result.unwrap_err();
    assert!(err.contains("put back what it had written"), "{err}");
    assert_eq!(snapshot(home.path()), before, "the memory entry is back");
    memory.close();
}

#[test]
fn the_wizard_offers_the_choice_and_its_answers_take_effect() {
    let home = TempDir::new("co-wiz-home");
    let data = TempDir::new("co-wiz-data");
    let work = TempDir::new("co-wiz-work");
    let project = mcp::canonical(work.path());
    named_machine(home.path());
    memory_fixture(home.path(), &project);
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    // Language: Enter; model: built-in (3); import: Enter; auto-memory: Enter; other memory:
    // 2 (switch it off); import its memories: 2 (no); summaries: 2; develop: 2; Enter applies.
    let (code, out) = run_wizard(&setup, &memory, "\n3\n\n\n2\n2\n2\n2\n\n");
    assert_eq!(code, 0, "{out}");
    for line in [
        "Step 5 of 8 \u{b7} Other memory",
        "- memory (Claude Code, ~/.claude.json): the reference memory server",
        "~/.claude/CLAUDE.md:4 says \"- Keep all memory in the knowledge graph",
        "1) graphersal-memory is the main memory",
        "2) Switch memory off",
        "3) Keep both as they are",
        "import 2 memories of memory now",
        "Step 8 of 8 \u{b7} Ready to install",
        "Other memory: memory switched off (uninstall switches it on again); 1 instruction file \
         line about memory elsewhere, never edited",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    assert!(config_json(&setup)["mcpServers"]["memory"].is_null());
    assert!(!out.contains("other memory: imported"), "{out}");
    // Again: switch it on again (2), then keep both is not asked (it is a yes/no now); the
    // entry comes back and is kept, not named (a known tool would be named: answer 2 = on).
    let (code, out) = run_wizard(&setup, &memory, "\n3\n\n2\n2\n\n\n\n");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("keep memory switched off"), "{out}");
    assert_eq!(
        config_json(&setup)["mcpServers"]["memory"],
        reference_entry(home.path())
    );
    assert_eq!(
        coexist::Choices::load(&setup).unwrap().read_only,
        ["memory"]
    );
    assert!(read(&setup.paths.instructions).contains("the MCP server `memory`"));
    memory.close();
}

#[test]
fn the_reference_server_and_basic_memory_formats_parse() {
    let items = coexist::other_import::parse_memory_server(concat!(
        r#"{"type":"entity","name":"Dark mode","entityType":"preference","observations":["Always dark"]}"#,
        "\n\nnot json\n",
        r#"{"type":"relation","from":"Dark mode","to":"Editor","relationType":"applies to"}"#,
        "\n"
    ));
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].slug, "kg-dark-mode");
    assert_eq!(items[0].kind, "user");
    assert_eq!(items[0].summary, "Always dark");
    assert_eq!(items[0].links, ["kg-editor"]);
    assert!(items[0].body.contains("- Dark mode applies to Editor"));
    // The older whole-file form.
    let old = coexist::other_import::parse_memory_server(
        r#"{"entities": [{"name": "X", "entityType": "thing", "observations": []}], "relations": []}"#,
    );
    assert_eq!(old[0].summary, "X (thing)");
    // Basic Memory: notes in folders, dot folders left out.
    let dir = TempDir::new("co-bm");
    std::fs::create_dir_all(dir.path().join("notes/.git")).unwrap();
    std::fs::write(
        dir.path().join("notes/coffee.md"),
        "---\ntitle: Coffee brewing\npermalink: coffee-brewing\n---\n\n# Coffee\n- [method] Pour over\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("notes/.git/x.md"), "no").unwrap();
    let items = coexist::other_import::parse_basic_memory(dir.path());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].slug, "basic-coffee-brewing");
    assert_eq!(items[0].summary, "Coffee brewing");
    assert!(items[0].body.contains("Pour over"));
}

#[test]
fn basic_memory_notes_are_imported_from_its_project_folders() {
    let home = TempDir::new("co-bmi-home");
    let data = TempDir::new("co-bmi-data");
    let h = home.path();
    std::fs::write(
        h.join(".claude.json"),
        r#"{"mcpServers": {"basic-memory": {"command": "uvx", "args": ["basic-memory", "mcp"]}}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(h.join(".basic-memory")).unwrap();
    let notes = h.join("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        h.join(".basic-memory/config.json"),
        json!({"projects": {"main": notes.display().to_string()}}).to_string(),
    )
    .unwrap();
    std::fs::write(
        notes.join("tea.md"),
        "---\ntitle: Tea\n---\nGreen tea at 80C.\n",
    )
    .unwrap();
    let setup = setup_in(h, data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("imported 1 memory (1 new"), "{out}");
    let found = memory.call("", "memory", json!({"slug": "basic-tea"}));
    assert_eq!(found["summary"], "Tea", "{found}");
    assert_eq!(found["source"], notes.join("tea.md").display().to_string());
    memory.close();
}

#[test]
fn instruction_lines_about_memory_elsewhere_are_found_and_never_edited() {
    let names = vec!["mem0".to_string(), "memory".to_string()];
    // Only names count, in any language; a paragraph naming graphersal-memory is ours.
    let text = "# Rules\n\
        - Write a MEMORY.md entry after each task.\n\
        - Speichere Fakten mit mem0, sobald du sie lernst.\n\
        - Keep memory usage under 1 GB.\n\
        - Храни память в MCP-сервере memory (create_entities).\n\
        - Remember to run the tests.\n\
        - Do not write memories into the KB any more; memory lives in graphersal-memory\n  \
          and MEMORY.md is only its export.\n\
        <!-- graphersal-memory:begin fingerprint=x -->\nKeep memory in MEMORY.md\n<!-- graphersal-memory:end -->\n\
        - Store memories in ~/notes/memory.txt.\n";
    let found: Vec<(usize, Option<String>)> = coexist::mentions_in(text, &names)
        .into_iter()
        .map(|(line, _, target)| (line, target))
        .collect();
    assert_eq!(
        found,
        vec![
            (2, Some("MEMORY.md".to_string())),
            (3, Some("mem0".to_string())),
            (5, Some("create_entities".to_string())),
        ]
    );
    // A project's own files are read when the project is given, and never changed.
    let home = TempDir::new("co-ins-home");
    let data = TempDir::new("co-ins-data");
    let work = TempDir::new("co-ins-work");
    named_machine(home.path());
    std::fs::write(
        work.path().join("CLAUDE.local.md"),
        "Write a MEMORY.md entry after each task.\n",
    )
    .unwrap();
    std::fs::write(
        work.path().join("AGENTS.md"),
        "Store project memory in notes/MEMORY.md.\n",
    )
    .unwrap();
    let setup = setup_in(home.path(), data.path());
    let found = coexist::detect(&setup, Some(work.path()));
    let files: Vec<String> = found
        .mentions
        .iter()
        .map(|m| m.file.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, ["CLAUDE.md", "CLAUDE.local.md", "AGENTS.md"]);
    assert_eq!(
        found.mentions[2].loaded_by,
        [Kind::Codex, Kind::OpenCode, Kind::Cursor]
    );
    let doctor = doctor_texts(&setup);
    let line = doctor
        .iter()
        .find(|t| t.contains("CLAUDE.md:4 says"))
        .expect("doctor shows the line");
    assert!(
        line.contains(
            "graphersal-memory never edits your instruction files: delete the line, or change it to"
        ),
        "{line}"
    );
    let memory = Memory::open(data.path());
    let before = read(&work.path().join("CLAUDE.local.md"));
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            project_dir: Some(work.path().to_path_buf()),
            ..yes()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(
            "Instruction files that mention other memory (never changed by graphersal-memory)"
        ),
        "{out}"
    );
    assert!(
        out.contains("CLAUDE.local.md:1: \"Write a MEMORY.md entry after each task.\""),
        "{out}"
    );
    assert_eq!(read(&work.path().join("CLAUDE.local.md")), before);
    memory.close();
}

/// Sets a file's modification time to `seconds` since the epoch.
fn set_mtime(path: &Path, seconds: u64) {
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
        .unwrap();
}

#[test]
fn external_writers_are_found_by_header_and_by_session_times() {
    let home = TempDir::new("co-wr-home");
    let data = TempDir::new("co-wr-data");
    let work = TempDir::new("co-wr-work");
    let project = mcp::canonical(work.path());
    let dir = memory_fixture(home.path(), &project);
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    // Written within the session's span: nobody else.
    assert!(coexist::writers(&setup).is_empty());
    // The only session ran two days ago for an hour; one file changed since.
    let now = coexist::parse_utc(&crate::daemon::now_utc()).unwrap();
    let start = now - 2 * 86400;
    let transcript = dir.parent().unwrap().join("0001.jsonl");
    std::fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({"cwd": project.display().to_string(), "type": "user",
                   "timestamp": crate::daemon::utc_timestamp(start)})
        ),
    )
    .unwrap();
    set_mtime(&transcript, start + 3600);
    for file in ["graph-first.md", "Odd_Name.md"] {
        set_mtime(&dir.join(file), start + 1800);
    }
    set_mtime(&dir.join("release-plan.md"), now - 60);
    let writers = coexist::writers(&setup);
    assert_eq!(writers.len(), 1, "{writers:?}");
    assert_eq!(writers[0].outside, 1);
    assert_eq!(writers[0].header, None);
    // A generated-file header names the program.
    std::fs::write(
        dir.join("MEMORY.md"),
        "<!-- Generated by sync.py from the KB; do not edit -->\n# Index\n",
    )
    .unwrap();
    let writers = coexist::writers(&setup);
    assert_eq!(
        writers[0].header.as_deref(),
        Some("<!-- Generated by sync.py from the KB; do not edit -->")
    );
    let doctor = doctor_texts(&setup).join("\n");
    assert!(
        doctor.contains("written by another program (MEMORY.md: \"<!-- Generated by sync.py"),
        "{doctor}"
    );
    assert!(
        doctor.contains("`graphersal-memory import --ignore "),
        "{doctor}"
    );
    memory.close();
}

#[test]
fn a_file_that_brings_back_a_forgotten_memory_is_reported_and_can_be_ignored() {
    let home = TempDir::new("co-ret-home");
    let data = TempDir::new("co-ret-data");
    let work = TempDir::new("co-ret-work");
    let project_dir = mcp::canonical(work.path());
    let dir = memory_fixture(home.path(), &project_dir);
    let project = project_dir.display().to_string();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    // Forgotten in the graph; another program rewrites its file.
    memory.call(&project, "forget", json!({"slug": "release-plan"}));
    let file = dir.join("release-plan.md");
    std::fs::write(
        &file,
        "---\nname: release-plan\ndescription: Releases go out on Thursdays\ntype: project\n---\n\nThursday.\n",
    )
    .unwrap();
    // And one changed by an agent in the graph, whose file changes too.
    memory.call(
        &project,
        "remember",
        json!({"slug": "graph-first", "kind": "feedback", "summary": "Graph first, always",
               "body": "Changed by the agent."}),
    );
    std::fs::write(
        dir.join("graph-first.md"),
        "---\nname: graph-first\ndescription: Study the graph first (old)\nmetadata:\n  type: feedback\n---\n\nOld text.\n",
    )
    .unwrap();
    let mut connect = memory.connector();
    let mut out = Vec::new();
    crate::commands::run_import(&setup, false, &mut connect, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("files that brought back a memory changed or forgotten in the graph: 2"),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "{} brought back the memory release-plan, which was forgotten in the graph",
            file.display()
        )),
        "{out}"
    );
    assert!(
        out.contains("brought back the memory graph-first, which was changed by an agent"),
        "{out}"
    );
    let found = coexist::returning(&setup);
    assert_eq!(found.len(), 2, "{found:?}");
    let doctor = doctor_texts(&setup).join("\n");
    assert!(
        doctor.contains("brought back the memory release-plan (forgotten in the graph"),
        "{doctor}"
    );
    assert!(
        doctor.contains("`graphersal-memory import --ignore ~/"),
        "{doctor}"
    );
    // Ignored: its next change is not imported; status names it; unignore imports again.
    let mut sink = Vec::new();
    crate::commands::ignore_import(&setup, &file, true, &mut sink).unwrap();
    assert!(
        String::from_utf8(sink)
            .unwrap()
            .contains("not imported any more")
    );
    std::fs::write(
        &file,
        "---\nname: release-plan\ndescription: Releases on Wednesdays\ntype: project\n---\n\nWed.\n",
    )
    .unwrap();
    let state = ImportState::load(data.path()).unwrap();
    let plans = import::plan(import::scan(&setup.paths.projects).unwrap(), &state);
    assert_eq!(plans[0].ignored, 1);
    assert!(plans[0].changed.iter().all(|f| f.path != file));
    let status = crate::commands::status_text(data.path(), &setup.run_dir, Ok(&setup)).unwrap();
    assert!(status.contains("not imported (ignored): ~/"), "{status}");
    let mut sink = Vec::new();
    crate::commands::ignore_import(&setup, &file, false, &mut sink).unwrap();
    let state = ImportState::load(data.path()).unwrap();
    assert!(!state.is_ignored(&file));
    memory.close();
}

#[test]
fn a_default_install_keeps_every_rule_the_user_had_in_context() {
    let home = TempDir::new("co-safe-home");
    let data = TempDir::new("co-safe-data");
    let work = TempDir::new("co-safe-work");
    let project_dir = mcp::canonical(work.path());
    let project = project_dir.display().to_string();
    // A realistic MEMORY.md: rules (feedback), facts about the user, pinned project facts and
    // other project facts; and CLAUDE.md rules of the user's own.
    let name = import::claude_slug(&project);
    let dir = home.path().join(".claude/projects").join(&name);
    let memory_dir = dir.join("memory");
    std::fs::create_dir_all(&memory_dir).unwrap();
    std::fs::write(
        dir.join("0001.jsonl"),
        format!("{}\n", json!({"cwd": project, "type": "user"})),
    )
    .unwrap();
    std::fs::write(
        memory_dir.join("MEMORY.md"),
        "## Pinned\n- [Release](release.md) — 0.6.0 released on 2026-11-10\n\n\
         ## How to work\n- [Tests need approval](tests-approval.md) — never change an existing test without asking\n\
         - [Watchdog](watchdog.md) — run cargo under the RSS watchdog\n\n\
         ## About you\n- [Role](role.md) — maintainer of the engine\n\n\
         ## Facts\n- [Disk](disk.md) — 30 GiB free\n",
    )
    .unwrap();
    for (file, kind, description) in [
        ("release.md", "project", "0.6.0 released on 2026-11-10"),
        (
            "tests-approval.md",
            "feedback",
            "Never change an existing test without asking",
        ),
        (
            "watchdog.md",
            "feedback",
            "Run cargo under the RSS watchdog",
        ),
        ("role.md", "user", "The user maintains the engine"),
        ("disk.md", "project", "About 30 GiB free"),
    ] {
        std::fs::write(
            memory_dir.join(file),
            format!(
                "---\nname: {}\ndescription: {description}\nmetadata:\n  type: {kind}\n---\n\n{description}.\n",
                file.trim_end_matches(".md")
            ),
        )
        .unwrap();
    }
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    let rules = "# My rules\n\n- Answer in English.\n- Never push without asking.\n";
    std::fs::write(home.path().join(".claude/CLAUDE.md"), rules).unwrap();
    let config = json!({"mcpServers": {"memory": reference_entry(home.path())}});
    std::fs::write(
        home.path().join(".claude.json"),
        format!("{}\n", serde_json::to_string_pretty(&config).unwrap()),
    )
    .unwrap();
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    // The default switched auto-memory off: the start context carries every rule and pinned
    // memory MEMORY.md carried, and CLAUDE.md keeps the user's rules (Claude Code loads it).
    let settings: Value = serde_json::from_str(&read(&setup.paths.settings)).unwrap();
    assert_eq!(settings["autoMemoryEnabled"], false);
    assert!(read(&setup.paths.instructions).starts_with(rules));
    let context = memory
        .runtime
        .start_context(&project, crate::start_context::DEFAULT_CHARS)
        .unwrap()
        .text;
    for slug in ["release", "tests-approval", "watchdog", "role"] {
        assert!(
            context.contains(&format!("(memory: {slug})")),
            "{slug} missing\n{context}"
        );
    }
    // A plain project fact is found by search, not loaded at every start (decided in M14).
    assert!(!context.contains("(memory: disk)"), "{context}");
    let disk = memory.call(&project, "memory", json!({"slug": "disk"}));
    assert!(disk.get("error").is_none(), "{disk}");
    // Uninstall gives back the exact previous state, the other tool's entry included (the
    // export of the memories back into the file memory is tested on its own).
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");
    memory.close();
}

#[test]
fn a_rule_block_naming_other_tools_is_ours_for_update_and_removal() {
    let english = crate::language::Language::English;
    let block = install::block_text_with(english, &["mem0".into()]);
    let text = format!("# Mine\n\n{block}");
    let found = install::find_block(&text).unwrap().unwrap();
    assert!(found.intact && found.known && !found.current, "{found:?}");
    let current = install::find_block_with(&text, english, &["mem0".into()])
        .unwrap()
        .unwrap();
    assert!(current.current);
    // The opencode, Codex and Cursor blocks too.
    let any = coexist::rule_with(
        crate::codex::install::rule_block(crate::language::Language::Any),
        &["a".into(), "b".into()],
    );
    assert!(any.ends_with(
        "- Other memory on this machine: the MCP servers `a` and `b`. graphersal-memory is the \
         main memory: search them only when you need to, never write there; save with \
         `remember` here.\n"
    ));
    let wrapped = install::wrap_block(&any);
    let found = crate::codex::install::find_block(&wrapped, english)
        .unwrap()
        .unwrap();
    assert!(found.known, "{found:?}");
    assert_eq!(
        coexist::strip_read_only(&any),
        crate::codex::install::rule_block(crate::language::Language::Any)
    );
    // A hand edit inside is still a conflict.
    let edited = text.replace("never write there", "write there");
    assert!(!install::find_block(&edited).unwrap().unwrap().known);
}

#[test]
fn the_switch_off_edits_undo_exactly() {
    let server = |harness: Kind, path: &[&str], entry: Value| coexist::Server {
        harness,
        name: "mem0".to_string(),
        file: PathBuf::from("/x"),
        path: path.iter().map(|p| p.to_string()).collect(),
        project: None,
        tool: None,
        why: String::new(),
        state: ServerState::On,
        entry,
    };
    // opencode 2.x: "disabled".
    let text = "{\n  \"mcp\": {\n    \"servers\": {\n      \"mem0\": {\"type\": \"local\", \"command\": [\"m\"]}\n    }\n  }\n}\n";
    let entry = json!({"type": "local", "command": ["m"]});
    let (off, record) =
        coexist::disable_edit(&server(Kind::OpenCode, &["mcp", "servers"], entry), text)
            .unwrap()
            .unwrap();
    assert_eq!(
        crate::jsonedit::value_of(&off).unwrap()["mcp"]["servers"]["mem0"]["disabled"],
        true
    );
    let back = coexist::restore_edit(Kind::OpenCode, &off, "mem0", &record)
        .unwrap()
        .unwrap();
    assert_eq!(back, text);
    // Switched on by the user meanwhile: nothing to do.
    let on = off.replace("\"disabled\": true", "\"disabled\": false");
    assert_eq!(
        coexist::restore_edit(Kind::OpenCode, &on, "mem0", &record).unwrap(),
        None
    );
    // Codex: a value that was there is put back.
    let text = "[mcp_servers.mem0]\ncommand = \"m\"\nenabled = true\n";
    let (off, record) = coexist::disable_edit(
        &server(
            Kind::Codex,
            &["mcp_servers"],
            json!({"command": "m", "enabled": true}),
        ),
        text,
    )
    .unwrap()
    .unwrap();
    assert!(off.contains("enabled = false"));
    assert_eq!(
        coexist::restore_edit(Kind::Codex, &off, "mem0", &record)
            .unwrap()
            .unwrap(),
        text
    );
    // A Codex entry written inline cannot be switched off in place.
    assert_eq!(
        coexist::disable_edit(
            &server(Kind::Codex, &["mcp_servers"], json!({"command": "m"})),
            "mcp_servers.mem0 = { command = \"m\" }\n"
        )
        .unwrap(),
        None
    );
}

#[test]
fn utc_times_parse() {
    assert_eq!(coexist::parse_utc("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(
        crate::daemon::utc_timestamp(coexist::parse_utc("2026-02-28T23:59:59Z").unwrap()),
        "2026-02-28T23:59:59Z"
    );
    assert_eq!(
        crate::daemon::utc_timestamp(coexist::parse_utc("2024-12-31T10:20:30.123Z").unwrap()),
        "2024-12-31T10:20:30Z"
    );
    assert_eq!(coexist::parse_utc("nope"), None);
}

#[test]
fn status_and_doctor_show_the_other_memory_section_without_actions() {
    let home = TempDir::new("co-st-home");
    let data = TempDir::new("co-st-data");
    named_machine(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    let status = crate::commands::status_text(data.path(), &setup.run_dir, Ok(&setup)).unwrap();
    let section = status
        .split("\nOther memory:\n")
        .nth(1)
        .expect("the section")
        .split("\n\n")
        .next()
        .unwrap();
    assert!(
        section.contains("Claude Code's auto-memory: off; 0 memory files in 0 projects"),
        "{section}"
    );
    assert!(
        section.contains(
            "memory (Claude Code, ~/.claude.json): the reference memory server \
             (@modelcontextprotocol/server-memory); named read-only in the rule block \
             (graphersal-memory is the main memory)"
        ),
        "{section}"
    );
    assert!(
        section.contains("~/.claude/CLAUDE.md:4 says: \"- Keep all memory in"),
        "{section}"
    );
    assert!(!status.contains("ACTION NEEDED"), "{status}");
    let findings = coexist::doctor_findings(&setup, &coexist::detect(&setup, None));
    assert!(findings.iter().all(|f| !f.problem), "{findings:?}");
    // A fresh machine without other memory shows no section.
    let home2 = TempDir::new("co-st-home2");
    let data2 = TempDir::new("co-st-data2");
    let setup2 = setup_in(home2.path(), data2.path());
    let status2 = crate::commands::status_text(data2.path(), &setup2.run_dir, Ok(&setup2)).unwrap();
    assert!(!status2.contains("Other memory:"), "{status2}");
    memory.close();
}

#[test]
fn the_export_leaves_out_memories_imported_from_another_tool() {
    let home = TempDir::new("co-exp-home");
    let data = TempDir::new("co-exp-data");
    named_machine(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    let row = |slug: &str| crate::export::Row {
        slug: slug.to_string(),
        kind: "feedback".to_string(),
        summary: format!("{slug} summary"),
        ..Default::default()
    };
    let rows = vec![row("kg-robin"), row("kg-graphersal"), row("mine")];
    let report = crate::export::write(&setup.agent(), data.path(), &rows).unwrap();
    assert_eq!(report.from_other_tools, 2);
    assert!(
        report
            .lines()
            .iter()
            .any(|l| l.contains("left out 2 memories imported from another memory tool")),
        "{:?}",
        report.lines()
    );
    let global = read(&setup.agent().global_memory_file());
    assert!(
        global.contains("mine summary") && !global.contains("kg-robin"),
        "{global}"
    );
    // Only memories of other tools: no global file at all.
    std::fs::remove_file(setup.agent().global_memory_file()).unwrap();
    let report = crate::export::write(&setup.agent(), data.path(), &rows[..2]).unwrap();
    assert_eq!(report.from_other_tools, 2);
    assert!(!setup.agent().global_memory_file().exists());
    memory.close();
}

#[test]
fn a_config_rewritten_while_install_waits_keeps_the_agents_write_and_the_switch_off() {
    let home = TempDir::new("co-d5-home");
    let data = TempDir::new("co-d5-data");
    named_machine(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let mut connect = memory.connector();
    let mut input = std::io::BufReader::new(crate::install_tests::RacingInput {
        path: setup.paths.user_config.clone(),
        answered: false,
    });
    let options = InstallOptions {
        yes: false,
        disable_other: vec!["memory".to_string()],
        ..yes()
    };
    let mut out = Vec::new();
    let code = install::install(&setup, &options, &mut connect, &mut input, &mut out).unwrap();
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&out));
    let config = config_json(&setup);
    assert_eq!(config["lastSessionId"], "abc", "the agent's write is kept");
    assert!(config["mcpServers"]["memory"].is_null(), "{config}");
    assert!(config["mcpServers"]["graphersal-memory"].is_object());
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    let config = config_json(&setup);
    assert_eq!(config["mcpServers"]["memory"], reference_entry(home.path()));
    assert_eq!(config["lastSessionId"], "abc");
    memory.close();
}

#[test]
fn an_instruction_wrapped_over_lines_is_read_as_one_paragraph() {
    // A list item wrapped over lines: one line alone mentions MEMORY.md, but the list item it
    // belongs to names graphersal-memory.
    let text = "## Memory\n\
        \n\
        - Do not write memories into the team wiki any more. Its `notes:*` pages and `drafts:*`\n  \
        index (folder `notes`, `scripts/notes/sync.py`) are the\n  \
        history up to 2026-11-11, readable as a fallback; `MEMORY.md` is still their export, imported\n  \
        into graphersal-memory at session start. Do not edit files in the memory directory.\n\
        - Never use tabs. Save every decision\n  \
        into mem0 right away.\n\
        - 每次任务后更新\n  \
        MEMORY.md。\n\
        \n\
        ```\n\
        claude-mem start\n\
        ```\n\
        | where | basic-memory |\n\
        A storage engine: the Store over `StoreDir` (directory, single file, memory).\n";
    let found: Vec<(usize, Option<String>)> = coexist::mentions_in(text, &["mem0".to_string()])
        .into_iter()
        .map(|(line, _, target)| (line, target))
        .collect();
    assert_eq!(
        found,
        vec![
            (8, Some("mem0".to_string())),
            (10, Some("MEMORY.md".to_string())),
            (13, Some("claude-mem".to_string())),
            (15, Some("basic-memory".to_string())),
        ]
    );
}

#[test]
fn claude_md_is_also_loaded_by_opencode_and_by_a_codex_fallback() {
    let home = TempDir::new("co-load-home");
    let data = TempDir::new("co-load-data");
    let work = TempDir::new("co-load-work");
    let h = home.path();
    std::fs::create_dir_all(h.join(".claude")).unwrap();
    std::fs::write(
        h.join(".claude/CLAUDE.md"),
        "- Keep all memory in Notion.\n",
    )
    .unwrap();
    std::fs::write(
        work.path().join("CLAUDE.md"),
        "- Write decisions into MEMORY.md.\n",
    )
    .unwrap();
    let setup = setup_in(h, data.path());
    let loaded = |setup: &Setup| -> Vec<(String, Vec<Kind>)> {
        coexist::instruction_files(setup, Some(work.path()))
            .into_iter()
            .map(|(p, kinds)| (p.display().to_string(), kinds))
            .collect()
    };
    let user_md = h.join(".claude/CLAUDE.md").display().to_string();
    let project_md = work.path().join("CLAUDE.md").display().to_string();
    // No opencode on the machine: Claude Code only.
    let files = loaded(&setup);
    assert!(
        files.contains(&(user_md.clone(), vec![Kind::ClaudeCode])),
        "{files:?}"
    );
    // opencode without its own AGENTS.md reads both CLAUDE.md files.
    std::fs::create_dir_all(h.join(".config/opencode")).unwrap();
    let files = loaded(&setup);
    let both = vec![Kind::ClaudeCode, Kind::OpenCode];
    assert!(
        files.contains(&(user_md.clone(), both.clone())),
        "{files:?}"
    );
    assert!(
        files.contains(&(project_md.clone(), both.clone())),
        "{files:?}"
    );
    // With its own AGENTS.md files it no longer reads them.
    std::fs::write(h.join(".config/opencode/AGENTS.md"), "# mine\n").unwrap();
    let files = loaded(&setup);
    assert!(
        files.contains(&(user_md.clone(), vec![Kind::ClaudeCode])),
        "{files:?}"
    );
    // Codex reads the project's CLAUDE.md when its config names it as a fallback.
    std::fs::create_dir_all(h.join(".codex")).unwrap();
    std::fs::write(
        h.join(".codex/config.toml"),
        "project_doc_fallback_filenames = [\"CLAUDE.md\"]\n",
    )
    .unwrap();
    let files = loaded(&setup);
    let entry = files.iter().find(|(p, _)| *p == project_md).unwrap();
    assert!(entry.1.contains(&Kind::Codex), "{files:?}");
    // The line is found once, naming every agent that loads it.
    let found = coexist::detect(&setup, Some(work.path()));
    let mention = found
        .mentions
        .iter()
        .find(|m| m.file == work.path().join("CLAUDE.md"))
        .unwrap();
    assert!(mention.loaded_by.contains(&Kind::Codex));
}
