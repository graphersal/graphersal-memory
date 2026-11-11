//! The MCP side: the two built-in profiles (`use`, `develop`), the project of a connection, an
//! open store served over MCP ([`StoreRuntime`]: the daemon holds one per store), and serving
//! the default store over stdio in this process (`graphersal-memory mcp --no-daemon`).
//!
//! The engine pieces: `graphersal-serve` runs the store's session on its own thread
//! (`spawn_session`, `Context`), `graphersal-mcp` speaks MCP (`Profile`, `Connection`,
//! `serve_lines`). The product adds the profiles, the instructions, the store and the project
//! binding: the memory functions take a `project` parameter, which each connection binds to its
//! project (`Profile::with_bound_params`): the engine hides it from the tool list and sets it on
//! every call, so an agent never passes (or fakes) it.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use graphersal::persist::Store;
use graphersal_mcp::{Mcp, McpConfig, Profile};
use graphersal_serve::{Context, Exposure, HostCheck, JobTicker, Runner};
use graphersal_session::{Session, SessionPresets};
use serde_json::{Value, json};

use crate::catalog;
use crate::cli::Profile as ProfileKind;
use crate::language::Language;
use crate::{registry, store};

/// The instructions of the `use` server.
pub const USE_INSTRUCTIONS: &str = include_str!("mcp/use.md");

/// The instructions of the `develop` server.
pub const DEVELOP_INSTRUCTIONS: &str = include_str!("mcp/develop.md");

/// The engine's built-in tools the `develop` server adds to the memory functions.
pub const DEVELOP_TOOLS: [&str; 14] = [
    "get_schema",
    "infer_schema",
    "validate_schema_patch",
    "diff_schema",
    "set_schema",
    "patch_schema",
    "list_saved_queries",
    "run_saved_query",
    "gql",
    "query",
    "statistics",
    "list_indexes",
    "create_index",
    "drop_index",
];

/// How long [`StoreRuntime::close`] waits for scheduled-job runs still in progress before it
/// closes the store anyway (a run finishing later drops its result).
const BACKGROUND_PATIENCE: Duration = Duration::from_secs(30);

/// The MCP configuration of a profile (what the agent sees), with the default memory language.
#[cfg(test)]
pub fn config(kind: ProfileKind) -> Result<McpConfig, String> {
    config_for(kind, Language::default())
}

/// The MCP configuration of a profile with the memory language rule `language` in the `use`
/// instructions.
pub fn config_for(kind: ProfileKind, language: Language) -> Result<McpConfig, String> {
    let value = match kind {
        ProfileKind::Use => json!({
            "server_name": "graphersal-memory",
            "server_title": "graphersal-memory",
            "instructions": {"mode": "replace",
                             "text": crate::language::instructions(USE_INSTRUCTIONS, language)},
            "tools": {"include": []},
            // `remember` and `forget` are meant to be called by the agent: their own
            // descriptions say what they write, without the engine's "only when the user asked".
            "saved_query_tools": {"names": catalog::FUNCTIONS, "write_note": "none"},
            "resources": [],
        }),
        ProfileKind::Develop => json!({
            "server_name": "graphersal-memory-develop",
            "server_title": "graphersal-memory (develop)",
            "instructions": {"mode": "replace", "text": DEVELOP_INSTRUCTIONS},
            "tools": {"include": DEVELOP_TOOLS},
            "saved_query_tools": {"names": catalog::FUNCTIONS, "write_note": "none"},
            "resources": ["schema-summary", "saved-queries", "gql-reference", "dsl-reference"],
        }),
    };
    McpConfig::parse(&value.to_string(), &mut |_| {
        Err("the built-in profiles read no files".to_string())
    })
    .map_err(|err| format!("the built-in MCP profile is invalid: {err}"))
}

/// The engine profile: the configuration, and for `use` the access policy built from the
/// memory functions' inferred effects (`develop` runs without a policy of its own: everything
/// is allowed).
pub fn profile(
    kind: ProfileKind,
    use_policy: graphersal::auth::AccessPolicy,
) -> Result<Profile, String> {
    profile_for(kind, use_policy, Language::default())
}

/// [`profile`] with the memory language `language`.
pub fn profile_for(
    kind: ProfileKind,
    use_policy: graphersal::auth::AccessPolicy,
    language: Language,
) -> Result<Profile, String> {
    let profile = Profile::new(false).with_config(config_for(kind, language)?);
    Ok(match kind {
        ProfileKind::Use => profile.with_access_policy(use_policy),
        ProfileKind::Develop => profile,
    })
}

/// The project of a session started in `cwd`: `flag` when given, else the root of the git
/// repository `cwd` is in (the nearest directory with a `.git` entry), else `cwd` itself.
/// Paths are made absolute and canonical where possible.
pub fn project_key(flag: Option<&Path>, cwd: &Path) -> String {
    let start = canonical(flag.unwrap_or(cwd));
    let root = if flag.is_some() {
        start
    } else {
        start
            .ancestors()
            .find(|dir| dir.join(".git").exists())
            .map(Path::to_path_buf)
            .unwrap_or(start)
    };
    root.display().to_string()
}

/// `path` canonicalized (without Windows' verbatim prefix), else as given.
pub(crate) fn canonical(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(path) => without_verbatim_prefix(path),
        Err(_) => path.to_path_buf(),
    }
}

/// `path` without Windows' verbatim prefix: `\\?\C:\x` is `C:\x`, `\\?\UNC\server\share` is
/// `\\server\share` (as Codex normalizes its home, `codex-rs/utils/absolute-path`).
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    let text = path.display().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => match rest.strip_prefix(r"UNC\") {
            Some(share) => PathBuf::from(format!(r"\\{share}")),
            None => PathBuf::from(rest),
        },
        None => path,
    }
}

/// What to serve.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// The data directory (the default store lives in it).
    pub data_dir: PathBuf,
    /// The profile of the connection.
    pub profile: ProfileKind,
    /// The project key every memory function is bound to.
    pub project: String,
}

/// Serves the default store of `options.data_dir` (created on first use) over newline-delimited
/// JSON-RPC on `read`/`write` until `read` ends; the store is closed before this returns.
pub fn serve(
    options: &ServeOptions,
    read: impl BufRead,
    write: impl Write + Send,
) -> Result<(), String> {
    let runtime = StoreRuntime::open_with(
        registry::DEFAULT_STORE,
        &store::default_store_dir(&options.data_dir),
        Arc::new(endpoint()),
        &crate::maintenance::Settings::load(&options.data_dir),
    )?;
    // The project's stable id (a moved repository keeps its memories); without a projects file
    // that can be read, the path itself.
    let project = crate::projects::resolve(&options.data_dir, &options.project)
        .map(|resolved| resolved.id)
        .unwrap_or_else(|_| options.project.clone());
    let language = crate::config::Config::load(&options.data_dir)
        .map(|config| config.language())
        .unwrap_or_default();
    let served = runtime.serve_with(options.profile, &project, None, language, read, write);
    let closed = runtime.close();
    served.and(closed)
}

/// The MCP endpoint every store's contexts share (its statistics and tool catalog are the
/// process's).
pub fn endpoint() -> Mcp {
    Mcp::new(false, None, false)
}

/// One open store served over MCP: its session thread (`graphersal-serve`'s runtime) and one
/// context per profile over that session, so connections of both profiles share the store's
/// single writer.
pub struct StoreRuntime {
    name: String,
    /// The store's directory.
    dir: PathBuf,
    /// The maintenance settings it was opened with.
    settings: crate::maintenance::Settings,
    store: Arc<Store>,
    using: Arc<Context>,
    developing: Arc<Context>,
    /// The `use` profile's access policy (a connection with another language rebuilds the
    /// profile with it).
    use_policy: graphersal::auth::AccessPolicy,
    /// The scheduled jobs' ticker ([`StoreRuntime::start_jobs`]), stopped by `close`.
    ticker: Mutex<Option<JobTicker>>,
    /// Held while the embedder switches (one switch at a time; the second sees the first's
    /// result).
    switching: Mutex<()>,
}

impl StoreRuntime {
    /// Opens (creates on first use) the store `name` in `dir` with the default maintenance
    /// settings, applies the product catalog and starts its session thread; `mcp` is the
    /// process's endpoint.
    #[cfg(test)]
    pub fn open(name: &str, dir: &Path, mcp: Arc<Mcp>) -> Result<Self, String> {
        Self::open_with(name, dir, mcp, &crate::maintenance::Settings::default())
    }

    /// [`Self::open`] with the maintenance `settings` (the checkpoint threshold).
    pub fn open_with(
        name: &str,
        dir: &Path,
        mcp: Arc<Mcp>,
        settings: &crate::maintenance::Settings,
    ) -> Result<Self, String> {
        let store = store::open_or_create_with(dir, settings.store_options())?;
        let served = store.clone();
        let (jobs, use_policy) = graphersal_serve::spawn_session(
            SessionPresets::new(),
            move |session: &mut Session| -> Result<_, String> {
                session
                    .attach_store(served, "memory")
                    .map_err(|err| err.to_string())?;
                catalog::apply(session)?;
                catalog::use_policy(session)
            },
            None,
        )
        .map_err(|err| format!("cannot start the memory store {}: {err}", dir.display()))?;
        let context = |kind: ProfileKind| -> Result<Arc<Context>, String> {
            Ok(Context::builder(
                Runner::Session(jobs.clone()),
                HostCheck::new(Exposure::Loopback, 0, ""),
            )
            .store(Some(store.clone()))
            .shared_mcp(mcp.clone())
            .mcp_profile(profile(kind, use_policy.clone())?)
            .build())
        };
        Ok(Self {
            name: name.to_string(),
            dir: dir.to_path_buf(),
            settings: *settings,
            using: context(ProfileKind::Use)?,
            developing: context(ProfileKind::Develop)?,
            use_policy,
            store,
            ticker: Mutex::new(None),
            switching: Mutex::new(()),
        })
    }

    /// The store's name in the registry.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The store's directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The open store.
    #[cfg(test)]
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// The store's maintenance numbers (`crate::maintenance::open_store_status`).
    pub fn maintenance_status(&self) -> Value {
        crate::maintenance::open_store_status(&self.store, &self.dir, &self.settings)
    }

    /// Prunes the history older than `keep` at `now_us` (`crate::maintenance::prune_old_history`).
    pub fn prune_old_history(
        &self,
        keep: Duration,
        now_us: i64,
    ) -> Result<Option<crate::maintenance::Pruned>, String> {
        crate::maintenance::prune_old_history(&self.store, &self.dir, keep, now_us)
    }

    /// The newest snapshot's commit.
    pub fn newest_snapshot(&self) -> Result<Option<u64>, String> {
        self.store
            .snapshots()
            .map(|snapshots| snapshots.last().map(|snapshot| snapshot.commit_seq))
            .map_err(|err| format!("cannot list the snapshots of {}: {err}", self.name))
    }

    /// The context of the profile `kind`.
    pub fn context(&self, kind: ProfileKind) -> &Arc<Context> {
        match kind {
            ProfileKind::Use => &self.using,
            ProfileKind::Develop => &self.developing,
        }
    }

    /// Ticks the store's scheduled jobs; their work runs on job worker threads, never on the
    /// session thread (`graphersal_serve::start_job_ticker`).
    pub fn start_jobs(&self) -> Result<(), String> {
        let ticker = graphersal_serve::start_job_ticker(&self.using, None)
            .map_err(|err| format!("cannot start the scheduled jobs of {}: {err}", self.name))?;
        // A ticker started before (none in the product) is detached by the replacement.
        *self.lock_ticker() = Some(ticker);
        Ok(())
    }

    fn lock_ticker(&self) -> std::sync::MutexGuard<'_, Option<JobTicker>> {
        self.ticker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The scheduled-job runs still in progress (handed to the job workers, queued or running);
    /// an error counts as none.
    pub fn background_runs(&self) -> usize {
        self.using.background_runs().unwrap_or(0)
    }

    /// Serves one MCP connection of profile `kind` bound to `project` over newline-delimited
    /// JSON-RPC on `read`/`write`, until `read` ends.
    #[cfg(test)]
    pub fn serve(
        &self,
        kind: ProfileKind,
        project: &str,
        identity: Option<&str>,
        read: impl BufRead,
        write: impl Write + Send,
    ) -> Result<(), String> {
        self.serve_with(kind, project, identity, Language::default(), read, write)
    }

    /// [`Self::serve`] with the memory language rule `language` in the instructions.
    pub fn serve_with(
        &self,
        kind: ProfileKind,
        project: &str,
        identity: Option<&str>,
        language: Language,
        read: impl BufRead,
        write: impl Write + Send,
    ) -> Result<(), String> {
        let connection = self
            .context(kind)
            .mcp_connection()
            .map_err(|err| err.to_string())?;
        let connection = match identity {
            Some(identity) => connection.with_identity(identity),
            None => connection,
        };
        let base = if language == Language::default() {
            connection.profile().clone()
        } else {
            profile_for(kind, self.use_policy.clone(), language)?
        };
        // The connection's project: hidden from the tool list, set on every call of a saved
        // query that declares it (the memory functions), whatever the client sent.
        let profile = base.with_bound_params([(
            catalog::PROJECT_PARAM.to_string(),
            Value::String(project.to_string()),
        )]);
        let connection = connection.with_profile(profile);
        graphersal_mcp::serve_lines(&connection, read, write)
            .map_err(|err| format!("the MCP stream failed: {}", err.message()))
    }

    /// Runs the catalog function `name` (an internal one: the session indexing) with `args` on
    /// the store's session thread.
    pub fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        let function = name.to_string();
        self.using
            .call(move |session| {
                let _doing =
                    crate::daemon::doing(format!("running the catalog function {function}"));
                crate::daemon::injected_panic("job", &function);
                catalog::call(session, &function, &args)
            })
            .map_err(|err| match err {
                graphersal_serve::ServeError::Session(_) => format!(
                    "the daemon failed while running {name} on the memory store {} (a bug)",
                    self.name
                ),
                other => format!("the memory store {}: {other}", self.name),
            })?
    }

    /// Whether the store's session thread still runs jobs.
    pub fn alive(&self) -> bool {
        self.using.call(|_| ()).is_ok()
    }

    /// The embedder the store's memory schema names (a model identity or `hash-ngram/1`).
    pub fn embedder(&self) -> Result<String, String> {
        self.using
            .call(|session| catalog::current_embedder(session))
            .map_err(|err| format!("the memory store {}: {err}", self.name))?
    }

    /// Makes the store's vector rules use `wanted` (a repository cached in `data_dir`'s model
    /// cache, or `hash-ngram/1`): the model is imported into the store from the cache (once;
    /// it becomes the store's default model) and the schema set again with it, which embeds
    /// every memory, session, turn and fact again in that unit (the store is locked while it
    /// runs: MCP calls wait). Nothing happens when the store uses `wanted` already. Returns
    /// whether it changed the embedder.
    pub fn switch_embedder(&self, data_dir: &Path, wanted: &str) -> Result<bool, String> {
        let _one_at_a_time = self
            .switching
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.embedder()?;
        let builtin = wanted == crate::models::BUILTIN;
        let same = if builtin {
            current == crate::models::BUILTIN
        } else {
            current.starts_with(&format!("model2vec/{wanted}@"))
        };
        if same {
            return Ok(false);
        }
        let held = self
            .store
            .models()
            .map_err(|err| format!("cannot list the models of {}: {err}", self.name))?;
        if builtin {
            if held.iter().any(|model| model.default) {
                self.store
                    .set_default_model(None)
                    .map_err(|err| format!("cannot clear the default model: {err}"))?;
            }
        } else if !held.iter().any(|model| model.repo == wanted) {
            let dir = crate::models::model_dir(data_dir, wanted);
            if crate::models::cached(data_dir, wanted).is_none() {
                return Err(format!(
                    "the embedding model {wanted} is not downloaded ({} is missing): run                      `graphersal-memory install --model {wanted}`",
                    dir.display()
                ));
            }
            self.store
                .import_model(
                    graphersal::persist::ModelSource::Dir(dir),
                    graphersal::persist::ImportModelOptions::new()
                        .with_default(true)
                        .with_principal("graphersal-memory"),
                )
                .map_err(|err| format!("cannot import the model {wanted}: {err}"))?;
        } else if !held
            .iter()
            .any(|model| model.repo == wanted && model.default)
        {
            self.store
                .set_default_model(Some(wanted))
                .map_err(|err| format!("cannot make {wanted} the default model: {err}"))?;
        }
        let schema = catalog::schema_with(wanted);
        let previous = catalog::schema_with(&current);
        let bare = catalog::schema_without_vectors()?;
        // Three steps on the session thread (no call runs in between): the rules go, the old
        // vectors go (another model may have another dimension, and the engine checks the
        // existing vectors against a new declaration before its fill), the new rules fill every
        // memory, session, turn and fact. A failure puts the old rules back (refilled). A crash
        // in between leaves no rules: the next start applies the catalog with the built-in
        // embedder and the daemon switches again.
        self.using
            .call(move |session| {
                let step = |session: &mut Session, text: &str| {
                    session
                        .set_schema(text, None)
                        .map(|_| ())
                        .map_err(|err| format!("the memory schema cannot be applied: {err}"))
                };
                step(session, &bare)?;
                let dropped: Value = serde_json::from_str(&session.execute(
                    "g.v().has(\"emb\").properties(\"emb\").drop().iterate()",
                    &graphersal_session::RunOptions {
                        no_limit: true,
                        ..Default::default()
                    },
                ))
                .unwrap_or(Value::Null);
                if dropped["ok"] != true {
                    let _ = step(session, &previous);
                    return Err(format!("cannot remove the old vectors: {dropped}"));
                }
                if let Err(err) = step(session, &schema) {
                    let _ = step(session, &previous);
                    return Err(err);
                }
                Ok(())
            })
            .map_err(|err| format!("the memory store {}: {err}", self.name))??;
        Ok(true)
    }

    /// Closes the store: the job ticker stops, the scheduled-job runs in progress finish (for
    /// up to [`BACKGROUND_PATIENCE`]), the session lets go of the store, then the store is shut
    /// down (WAL synced, marked closed cleanly, its lock released), so a later open finds it
    /// unlocked.
    pub fn close(&self) -> Result<(), String> {
        let ticker = self.lock_ticker().take();
        let stopped = match ticker {
            Some(ticker) => ticker
                .stop()
                .map_err(|err| format!("cannot stop the scheduled jobs of {}: {err}", self.name)),
            None => Ok(()),
        };
        let deadline = Instant::now() + BACKGROUND_PATIENCE;
        while self.background_runs() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let released = self
            .using
            .call(|session| *session = Session::new())
            .map_err(|err| format!("cannot close the memory store: {err}"));
        let shut = self
            .store
            .shutdown()
            .map_err(|err| format!("cannot close the memory store {}: {err}", self.name));
        stopped.and(released).and(shut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_profiles_parse() {
        let using = config(ProfileKind::Use).unwrap();
        assert!(using.has_saved_query_tools());
        assert!(
            config(ProfileKind::Develop)
                .unwrap()
                .has_saved_query_tools()
        );
        assert!(
            USE_INSTRUCTIONS.len() < 2000,
            "keep the use instructions short"
        );
    }

    #[test]
    fn the_project_is_the_repository_root() {
        let dir =
            std::env::temp_dir().join(format!("graphersal-memory-project-{}", std::process::id()));
        let nested = dir.join("repo").join("src").join("deep");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(dir.join("repo").join(".git")).unwrap();
        let root = canonical(&dir.join("repo")).display().to_string();
        assert_eq!(project_key(None, &nested), root);
        let flagged = project_key(Some(&nested), &dir);
        assert_eq!(flagged, canonical(&nested).display().to_string());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_verbatim_prefix_is_dropped() {
        let strip = |text: &str| {
            without_verbatim_prefix(PathBuf::from(text))
                .display()
                .to_string()
        };
        assert_eq!(strip(r"\\?\C:\Users\u"), r"C:\Users\u");
        assert_eq!(strip(r"\\?\UNC\server\share\x"), r"\\server\share\x");
        assert_eq!(strip("/home/u"), "/home/u");
    }
}
