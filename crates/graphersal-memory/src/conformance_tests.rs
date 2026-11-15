//! The conformance suite: what every harness must do, run for each one of `Kind::ALL` from its
//! own user files (`Harness::user_files`), alone on a machine with only its program. A new
//! harness gets these tests by its registry line and its `user_files`; its own tests cover
//! only what is particular to it.

use std::path::Path;

use crate::agents::Kind;
use crate::install::{self, InstallOptions, Setup, UninstallOptions};
use crate::install_tests::{
    Memory, bare_setup_in, refresh, run_install, run_uninstall, snapshot, with_programs,
};
use crate::mcp_tests::TempDir;

/// A machine with only `kind`'s program and its user files in `home`.
fn machine(kind: Kind, home: &Path, data: &Path) -> Setup {
    // The files first: a harness's paths are resolved from what is there (opencode's
    // `opencode.jsonc` or `opencode.json`), as when a command starts.
    bare_setup_in(home, data).agent_of(kind).user_files(home);
    with_programs(bare_setup_in(home, data), &[kind.program_name()])
}

/// Install into `kind` only, without questions.
fn into(kind: Kind) -> InstallOptions {
    InstallOptions {
        harnesses: Some(vec![kind]),
        yes: true,
        ..InstallOptions::default()
    }
}

/// Uninstall from `kind` only, without questions and without the export.
fn out_of(kind: Kind) -> UninstallOptions {
    UninstallOptions {
        harnesses: Some(vec![kind]),
        yes: true,
        no_export: true,
        ..UninstallOptions::default()
    }
}

/// Runs `check` for every harness, each on fresh temporary directories.
fn each(tag: &str, check: impl Fn(Kind, &Path, &Path)) {
    for kind in Kind::ALL {
        let home = TempDir::new(&format!("conf-{tag}-{}-home", kind.id()));
        let data = TempDir::new(&format!("conf-{tag}-{}-data", kind.id()));
        check(kind, home.path(), data.path());
    }
}

#[test]
fn install_writes_its_parts_and_uninstall_gives_every_file_back_byte_for_byte() {
    each("rt", |kind, home, data| {
        let setup = machine(kind, home, data);
        let before = snapshot(home);
        let memory = Memory::open(data);
        let (code, out) = run_install(&setup, &memory, into(kind));
        assert_eq!(code, 0, "{kind:?}: {out}");
        assert_ne!(snapshot(home), before, "{kind:?}: install wrote nothing");
        assert!(setup.agent_of(kind).has_our_parts(&setup), "{kind:?}");
        assert!(setup.installed().contains(&kind), "{kind:?}");
        let (code, out) = run_uninstall(&setup, out_of(kind));
        assert_eq!(code, 0, "{kind:?}: {out}");
        assert_eq!(snapshot(home), before, "{kind:?}: {out}");
        assert!(!setup.agent_of(kind).has_our_parts(&setup), "{kind:?}");
        memory.close();
    });
}

#[test]
fn a_second_install_and_a_refresh_change_nothing() {
    each("again", |kind, home, data| {
        let setup = machine(kind, home, data);
        let memory = Memory::open(data);
        let (code, out) = run_install(&setup, &memory, into(kind));
        assert_eq!(code, 0, "{kind:?}: {out}");
        let installed = snapshot(home);
        let (code, out) = run_install(&setup, &memory, into(kind));
        assert_eq!(code, 0, "{kind:?}: {out}");
        assert_eq!(snapshot(home), installed, "{kind:?}: {out}");
        let (code, out) = run_install(&setup, &memory, refresh());
        assert_eq!(code, 0, "{kind:?}: {out}");
        assert!(out.contains("up to date"), "{kind:?}: {out}");
        assert_eq!(snapshot(home), installed, "{kind:?}: {out}");
        memory.close();
    });
}

#[test]
fn a_dry_run_writes_nothing() {
    each("dry", |kind, home, data| {
        let setup = machine(kind, home, data);
        let before = snapshot(home);
        let memory = Memory::open(data);
        let dry = InstallOptions {
            dry_run: true,
            ..into(kind)
        };
        let (code, out) = run_install(&setup, &memory, dry);
        assert_eq!(code, 0, "{kind:?}: {out}");
        assert_eq!(snapshot(home), before, "{kind:?}: {out}");
        assert!(!install::State::exists_of(&setup, kind), "{kind:?}");
        memory.close();
    });
}

#[test]
fn status_names_the_installed_parts_and_none_after_uninstall() {
    each("status", |kind, home, data| {
        let setup = machine(kind, home, data);
        let memory = Memory::open(data);
        run_install(&setup, &memory, into(kind));
        let lines = setup.agent_of(kind).status_lines(&setup).join("\n");
        assert!(lines.contains("installed"), "{kind:?}: {lines}");
        let findings = setup.agent_of(kind).doctor(&setup, None);
        assert!(!findings.is_empty(), "{kind:?}");
        run_uninstall(&setup, out_of(kind));
        assert!(!setup.installed().contains(&kind), "{kind:?}");
        memory.close();
    });
}

#[test]
fn a_configuration_without_its_program_is_a_leftover_never_offered() {
    each("left", |kind, home, data| {
        bare_setup_in(home, data).agent_of(kind).user_files(home);
        let setup = bare_setup_in(home, data);
        assert!(!setup.detected().contains(&kind), "{kind:?}");
        assert!(
            setup.leftovers().iter().any(|(k, _)| *k == kind),
            "{kind:?}: {:?}",
            setup.leftovers()
        );
    });
}

#[test]
fn the_develop_server_goes_on_and_off_and_the_rest_stays() {
    each("dev", |kind, home, data| {
        let setup = machine(kind, home, data);
        let before = snapshot(home);
        let memory = Memory::open(data);
        let develop = InstallOptions {
            enable_develop: true,
            ..into(kind)
        };
        let (code, out) = run_install(&setup, &memory, develop);
        assert_eq!(code, 0, "{kind:?}: {out}");
        assert!(setup.agent_of(kind).develop_on(&setup), "{kind:?}: {out}");
        let off = UninstallOptions {
            disable_develop_only: true,
            ..out_of(kind)
        };
        let (code, out) = run_uninstall(&setup, off);
        assert_eq!(code, 0, "{kind:?}: {out}");
        assert!(!setup.agent_of(kind).develop_on(&setup), "{kind:?}: {out}");
        assert!(setup.agent_of(kind).has_our_parts(&setup), "{kind:?}");
        let (code, out) = run_uninstall(&setup, out_of(kind));
        assert_eq!(code, 0, "{kind:?}: {out}");
        assert_eq!(snapshot(home), before, "{kind:?}: {out}");
        memory.close();
    });
}

#[cfg(unix)]
#[test]
fn a_read_only_configuration_is_refused_and_nothing_is_written() {
    use std::os::unix::fs::PermissionsExt;
    each("ro", |kind, home, data| {
        let setup = machine(kind, home, data);
        // Every file of the user read-only: install changes at least one of them.
        let files: Vec<_> = snapshot(home)
            .into_iter()
            .map(|(path, _)| path)
            .filter(|path| home.join(path).is_file())
            .collect();
        let mode = |path: &Path, mode: u32| {
            std::fs::set_permissions(home.join(path), std::fs::Permissions::from_mode(mode))
                .unwrap();
        };
        for path in &files {
            mode(path, 0o444);
        }
        let before = snapshot(home);
        let memory = Memory::open(data);
        let mut connect = memory.connector();
        let mut out = Vec::new();
        let result = install::install(&setup, &into(kind), &mut connect, &mut &b""[..], &mut out);
        let err = result.expect_err(&format!("{kind:?}: a read-only file is refused"));
        assert!(err.contains("nothing was changed"), "{kind:?}: {err}");
        assert_eq!(snapshot(home), before, "{kind:?}");
        for path in &files {
            mode(path, 0o644);
        }
        memory.close();
    });
}
