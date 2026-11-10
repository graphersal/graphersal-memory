// COPIED from graphersal crates/graphersal-cli/src/embed_model.rs (graphersal 0.6.0, 494f5da2;
// the file last changed in ada7edea): the Model2Vec download. If you change it here, check
// whether the original needs the same change, and vice versa (graphersal-cli embed-model
// download).
//
//! The embedding model download of `install`: the one network access of install. It fetches
//! the three files of a Model2Vec model (`config.json`, `tokenizer.json`, `model.safetensors`)
//! from `<endpoint>/<repo>/resolve/<revision>/<file>` (endpoint `https://huggingface.co`, or
//! `GRAPHERSAL_MEMORY_HF_ENDPOINT` for a mirror or a test; `HF_TOKEN` as a bearer token, sent to
//! that endpoint's host only, never logged). Each file streams into a `.part` file of a hidden
//! staging directory while its SHA-256 is computed; the hash Hugging Face states for an LFS file
//! (`X-Linked-Etag`, or a git blob id in the `ETag`) and the size are checked, the file renamed;
//! the manifest (`graphersal::persist::ModelCache::MANIFEST_FILE`) is written, the model loaded
//! once as a check, and only then the staging directory renamed to `<models dir>/<repo>`. A
//! failure (or a kill) leaves no model behind: at most a hidden staging directory, which the
//! next download removes.
//!
//! The model's licence travels with it: the download also fetches the model card
//! (`README.md`), the repository's `LICENSE*`/`NOTICE*` files and its metadata
//! (`<endpoint>/api/models/<repo>/revision/<revision>`: `cardData.license`,
//! `cardData.base_model`, `gated`), all recorded in the manifest; the import into a store
//! (`graphersal::persist::Store::import_model`, `crate::mcp::StoreRuntime::switch_embedder`)
//! carries them into the store. Warnings name an unknown, non-commercial or custom licence and
//! a gated repository.
//!
//! Differences from the original: no `--into` (the daemon imports the cached model into the
//! open store), progress and warnings go to the caller's writer instead of stderr, and errors
//! are plain strings.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use graphersal::persist::ModelCache;
use serde_json::{Value, json};

/// The files a Model2Vec model in the Model2Vec layout consists of, in download order.
pub const MODEL_FILES: [&str; 3] = ["config.json", "tokenizer.json", "model.safetensors"];

/// The default Hugging Face endpoint.
pub const DEFAULT_ENDPOINT: &str = "https://huggingface.co";

/// The environment variable naming another endpoint (a mirror, a test server).
pub const ENDPOINT_ENV: &str = "GRAPHERSAL_MEMORY_HF_ENDPOINT";

/// The largest file a download accepts when the server states no size.
const MAX_FILE_BYTES: u64 = 8 << 30;

/// The largest metadata answer read.
const MAX_METADATA_BYTES: u64 = 8 << 20;

/// The model card's file name.
const CARD_FILE: &str = "README.md";

/// Model roles as the manifest names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Config,
    Tokenizer,
    Weights,
    Card,
    License,
    Notice,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Config => "config",
            Role::Tokenizer => "tokenizer",
            Role::Weights => "weights",
            Role::Card => "card",
            Role::License => "license",
            Role::Notice => "notice",
        }
    }
}

/// Where and how a download connects.
#[derive(Debug, Clone)]
pub struct Source {
    /// The endpoint (no trailing slash).
    pub endpoint: String,
    /// A Hugging Face token for gated or private repositories.
    pub token: Option<String>,
}

impl Source {
    /// The endpoint of the environment (`GRAPHERSAL_MEMORY_HF_ENDPOINT`, else Hugging Face) and
    /// the token `HF_TOKEN`.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Self {
        let set = |name: &str| env(name).filter(|v| !v.trim().is_empty());
        Source {
            endpoint: set(ENDPOINT_ENV)
                .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string())
                .trim_end_matches('/')
                .to_string(),
            token: set("HF_TOKEN"),
        }
    }
}

/// A finished download.
#[derive(Debug, Clone)]
pub struct Downloaded {
    /// Where the model is now.
    pub dir: PathBuf,
    /// The licence (an SPDX id), `unknown` when not stated.
    pub license: String,
    /// The bytes of the three model files.
    pub bytes: u64,
    /// The model identity (`model2vec/<repo>@<12 hex>`).
    pub identity: String,
    /// The licence warnings (unknown, non-commercial, custom terms, gated).
    pub warnings: Vec<String>,
}

/// Whether `repo` is a Hugging Face repository name: `owner/name` (or a bare `name`), each part
/// of ASCII letters, digits, `.`, `_`, `-`, not starting with `.`.
pub fn is_repo_name(repo: &str) -> bool {
    let part_ok = |part: &str| {
        !part.is_empty()
            && part.len() <= 96
            && !part.starts_with('.')
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    let mut parts = repo.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(name), None, None) => part_ok(name),
        (Some(owner), Some(name), None) => part_ok(owner) && part_ok(name),
        _ => false,
    }
}

/// Bytes for people: `130.0 MiB`.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// One downloaded file.
struct Fetched {
    name: String,
    size: u64,
    sha256: String,
    /// The commit the server resolved the revision to (`X-Repo-Commit`).
    commit: Option<String>,
}

/// A staging directory removed on drop unless it was moved into place.
struct Staging {
    path: PathBuf,
    done: bool,
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.done {
            let _ = fs::remove_dir_all(&self.path);
            // The owner directory a failed first download created (removed only when empty).
            if let Some(parent) = self.path.parent() {
                let _ = fs::remove_dir(parent);
            }
        }
    }
}

/// Downloads `repo` (revision `main`) from `source` into `models/<repo>`; progress lines go to
/// `out` (rewritten in place when `live`: a terminal). Returns where it is and its licence.
pub fn download(
    repo: &str,
    models: &Path,
    source: &Source,
    out: &mut dyn Write,
    live: bool,
) -> Result<Downloaded, String> {
    let revision = "main";
    if !is_repo_name(repo) {
        return Err(format!(
            "'{repo}' is not a Hugging Face repository name: owner/name of letters, digits, \
             '.', '_' and '-' (for example minishlab/potion-base-8M)"
        ));
    }
    let endpoint = source.endpoint.trim_end_matches('/').to_string();
    let target = models.join(repo);
    let parent = target
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| models.to_path_buf());
    fs::create_dir_all(&parent)
        .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
    let leaf = repo.rsplit('/').next().unwrap_or(repo);
    remove_stale_staging(&parent, leaf);
    let mut staging = Staging {
        path: parent.join(format!(".{leaf}.download-{}", std::process::id())),
        done: false,
    };
    let _ = fs::remove_dir_all(&staging.path);
    fs::create_dir_all(&staging.path)
        .map_err(|err| format!("cannot create {}: {err}", staging.path.display()))?;

    let _ = writeln!(out, "  Downloading {repo} from {endpoint}");
    let client = Client::new(&endpoint, source.token.clone());
    let encoded_revision = revision.replace('/', "%2F");
    let mut fetched: Vec<(Fetched, Option<Role>)> = Vec::new();
    for name in MODEL_FILES {
        let url = format!("{endpoint}/{repo}/resolve/{encoded_revision}/{name}");
        let file = client.fetch(&url, repo, revision, name, &staging.path, false, out, live)?;
        fetched.push((file.ok_or_else(|| format!("{repo} has no {name}"))?, None));
    }
    let commit = fetched.iter().find_map(|(f, _)| f.commit.clone());

    // What travels with the model: its metadata, card and licence files.
    let meta = client.metadata(&endpoint, repo, Some(&encoded_revision));
    let mut warnings = Vec::new();
    if let Some(note) = &meta.note {
        warnings.push(note.clone());
    }
    let card_url = format!("{endpoint}/{repo}/resolve/{encoded_revision}/{CARD_FILE}");
    if let Some(card) = client.fetch(
        &card_url,
        repo,
        revision,
        CARD_FILE,
        &staging.path,
        true,
        out,
        live,
    )? {
        fetched.push((card, Some(Role::Card)));
    }
    for (name, role) in licence_files(&meta.siblings) {
        let url = format!("{endpoint}/{repo}/resolve/{encoded_revision}/{name}");
        let file = client
            .fetch(&url, repo, revision, &name, &staging.path, false, out, live)?
            .ok_or_else(|| format!("{repo} lists {name} but does not serve it"))?;
        fetched.push((file, Some(role)));
    }
    let base_models: Vec<(String, Option<String>)> = meta
        .base_models
        .iter()
        .take(4)
        .map(|base| (base.clone(), client.metadata(&endpoint, base, None).license))
        .collect();
    let license = meta
        .license
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let has_licence_file = fetched.iter().any(|(_, role)| *role == Some(Role::License));
    let manifest = json!({
        "format": 2,
        "family": "model2vec",
        "repo": repo,
        "revision": revision,
        "commit": commit,
        "license": license,
        "license_text": if has_licence_file { "file" } else { "none" },
        "base_models": base_models.iter().map(|(repo, license)| match license {
            Some(license) => json!({"repo": repo, "license": license}),
            None => json!({"repo": repo}),
        }).collect::<Vec<_>>(),
        "gated": meta.gated,
        "files": fetched.iter().map(|(f, role)| json!({
            "name": f.name,
            "role": role.map_or_else(|| model_role(&f.name).as_str(), Role::as_str),
            "size": f.size,
            "sha256": f.sha256,
        })).collect::<Vec<_>>(),
        "downloaded_at": now_text(),
    });
    let manifest_path = staging.path.join(ModelCache::MANIFEST_FILE);
    write_synced(
        &manifest_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&manifest).unwrap_or_default()
        )
        .as_bytes(),
    )
    .map_err(|err| format!("cannot write {}: {err}", manifest_path.display()))?;

    // Load it once: a repository that is not a Model2Vec model fails here, not later.
    let identity = ModelCache::check(&staging.path).map_err(|err| {
        format!("{repo} was downloaded but does not load as a Model2Vec model: {err}")
    })?;
    warnings.extend(licence_warnings(repo, &license, meta.gated));
    let bytes = fetched
        .iter()
        .filter(|(_, role)| role.is_none())
        .map(|(f, _)| f.size)
        .sum();

    install(&staging, &target)?;
    staging.done = true;
    Ok(Downloaded {
        dir: target,
        license,
        bytes,
        identity,
        warnings,
    })
}

/// The role of one of the three model files by its name.
fn model_role(name: &str) -> Role {
    match name {
        "tokenizer.json" => Role::Tokenizer,
        "model.safetensors" => Role::Weights,
        _ => Role::Config,
    }
}

/// The licence and notice files of a repository's file list: `LICENSE*`, `LICENCE*`,
/// `COPYING*` (role licence) and `NOTICE*` (role notice) at its root, names a Store accepts.
fn licence_files(siblings: &[String]) -> Vec<(String, Role)> {
    siblings
        .iter()
        .filter(|name| !name.contains('/') && is_plain_file_name(name))
        .filter_map(|name| {
            let upper = name.to_ascii_uppercase();
            if upper.starts_with("LICENSE")
                || upper.starts_with("LICENCE")
                || upper.starts_with("COPYING")
            {
                Some((name.clone(), Role::License))
            } else if upper.starts_with("NOTICE") {
                Some((name.clone(), Role::Notice))
            } else {
                None
            }
        })
        .take(8)
        .collect()
}

/// A file name a Store keeps in a model directory.
fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && !name.starts_with('.')
        && !name.starts_with("tmp-")
        && name != ModelCache::MANIFEST_FILE
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The warnings about passing a model on: an unknown or non-commercial licence, custom terms,
/// a gated repository.
fn licence_warnings(repo: &str, license: &str, gated: bool) -> Vec<String> {
    let mut warnings = Vec::new();
    let lower = license.to_ascii_lowercase();
    if lower == "unknown" {
        warnings.push(format!(
            "the licence of {repo} is unknown (its model card names none, or its metadata could \
             not be read): check the repository before you hand on a store that carries the \
             model"
        ));
    } else if lower.contains("-nc")
        || lower.contains("noncommercial")
        || lower.contains("non-commercial")
    {
        warnings.push(format!(
            "{repo} is under {license}, a NON-COMMERCIAL licence: a store carrying it may not be \
             used or handed on commercially"
        ));
    } else if lower == "other" {
        warnings.push(format!(
            "{repo} is under custom terms (licence \"other\"): read its model card before you \
             hand on a store that carries the model"
        ));
    }
    if gated {
        warnings.push(format!(
            "{repo} is a GATED repository: the terms you accepted to download it may restrict \
             passing it on"
        ));
    }
    warnings
}

/// Moves the finished staging directory to `target`, replacing an older download.
fn install(staging: &Staging, target: &Path) -> Result<(), String> {
    let old = target.with_file_name(format!(
        ".{}.old-{}",
        target
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        std::process::id()
    ));
    let replaced = target.exists();
    if replaced {
        fs::rename(target, &old).map_err(|err| {
            format!(
                "cannot replace the earlier download {}: {err}",
                target.display()
            )
        })?;
    }
    if let Err(err) = fs::rename(&staging.path, target) {
        if replaced {
            let _ = fs::rename(&old, target);
        }
        return Err(format!(
            "cannot move the model to {}: {err}",
            target.display()
        ));
    }
    if replaced {
        let _ = fs::remove_dir_all(&old);
    }
    if let Some(parent) = target.parent() {
        sync_dir(parent);
    }
    Ok(())
}

/// Removes the leftovers of interrupted downloads of `leaf` in `parent`.
fn remove_stale_staging(parent: &Path, leaf: &str) {
    let prefixes = [format!(".{leaf}.download-"), format!(".{leaf}.old-")];
    for entry in fs::read_dir(parent).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if prefixes.iter().any(|p| name.starts_with(p.as_str())) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = fs::File::open(dir) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Now, as the canonical UTC date-time text.
fn now_text() -> String {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    graphersal::temporal::DateTime::from_micros(micros - micros % 1_000_000, 0)
        .map(|t| t.to_string())
        .unwrap_or_default()
}

/// What the Hugging Face API says about a repository (the parts a Store records).
#[derive(Debug, Default)]
struct HubMetadata {
    /// `cardData.license` (an SPDX id), when stated.
    license: Option<String>,
    /// `cardData.base_model` (a string or a list).
    base_models: Vec<String>,
    /// `gated`: `false`, or `"auto"`/`"manual"`.
    gated: bool,
    /// The repository's files (`siblings[].rfilename`).
    siblings: Vec<String>,
    /// Why the metadata is missing, when it is.
    note: Option<String>,
}

impl HubMetadata {
    fn from_json(value: &Value) -> Self {
        let card = value.get("cardData");
        let license = card
            .and_then(|c| c.get("license"))
            .and_then(|l| match l {
                Value::String(text) => Some(text.clone()),
                Value::Array(items) => items.first().and_then(Value::as_str).map(str::to_string),
                _ => None,
            })
            .filter(|l| {
                !l.trim().is_empty() && l.len() <= 64 && l.chars().all(|c| c.is_ascii_graphic())
            });
        let base_models = match card.and_then(|c| c.get("base_model")) {
            Some(Value::String(base)) => vec![base.clone()],
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        }
        .into_iter()
        .filter(|base| is_repo_name(base))
        .collect();
        let gated = match value.get("gated") {
            Some(Value::Bool(gated)) => *gated,
            Some(Value::String(_)) => true,
            _ => false,
        };
        let siblings = value
            .get("siblings")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|s| s.get("rfilename").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        HubMetadata {
            license,
            base_models,
            gated,
            siblings,
            note: None,
        }
    }
}

/// The HTTP client of a download.
struct Client {
    agent: ureq::Agent,
    /// The authority (`host[:port]`) of the endpoint: the only one `HF_TOKEN` is sent to.
    endpoint_authority: String,
    token: Option<String>,
}

/// The `scheme://authority` part and the authority of an absolute URL.
fn origin_of(url: &str) -> Option<(&str, &str)> {
    let scheme_end = url.find("://")?;
    let rest = &url[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let origin_len = scheme_end + 3 + authority_end;
    Some((&url[..origin_len], &rest[..authority_end]))
}

/// `location` (a redirect target) resolved against `base`.
fn resolve_location(base: &str, location: &str) -> Option<String> {
    if location.starts_with("http://") || location.starts_with("https://") {
        return Some(location.to_string());
    }
    let (origin, _) = origin_of(base)?;
    if let Some(rest) = location.strip_prefix("//") {
        let scheme = &base[..base.find("://")?];
        return Some(format!("{scheme}://{rest}"));
    }
    if location.starts_with('/') {
        return Some(format!("{origin}{location}"));
    }
    let path = base[origin.len()..].split(['?', '#']).next().unwrap_or("/");
    let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
    Some(format!("{origin}{dir}{location}"))
}

/// The header `name` of a response, as text.
fn header(response: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// The hash a server states for a file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stated {
    /// The SHA-256 of the content (Hugging Face LFS files: `X-Linked-Etag`).
    Sha256(String),
    /// The git blob id: SHA-1 of `blob <size>\0` + the content (Hugging Face files kept in git).
    GitSha1(String),
}

/// The hash inside an ETag value (`"<hex>"`, `W/"<hex>"`): 64 hex digits are a SHA-256, 40 a
/// git blob id; anything else is not a content hash.
fn etag_hash(value: &str) -> Option<Stated> {
    let hex = value.trim().trim_start_matches("W/").trim_matches('"');
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        64 => Some(Stated::Sha256(hex.to_lowercase())),
        40 => Some(Stated::GitSha1(hex.to_lowercase())),
        _ => None,
    }
}

impl Client {
    fn new(endpoint: &str, token: Option<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(120)))
            .user_agent(concat!("graphersal-memory/", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent();
        Client {
            agent,
            endpoint_authority: origin_of(endpoint)
                .map(|(_, authority)| authority.to_string())
                .unwrap_or_default(),
            token,
        }
    }

    /// The Hugging Face metadata of `repo` (at `revision`, URL-encoded; `None`: the default
    /// branch): `cardData.license`, `cardData.base_model`, `gated` and the file list. Best
    /// effort: an unreachable or unreadable answer gives empty metadata and a note.
    fn metadata(&self, endpoint: &str, repo: &str, revision: Option<&str>) -> HubMetadata {
        let url = match revision {
            Some(revision) => format!("{endpoint}/api/models/{repo}/revision/{revision}"),
            None => format!("{endpoint}/api/models/{repo}"),
        };
        let mut request = self.agent.get(&url);
        if origin_of(&url).is_some_and(|(_, a)| a == self.endpoint_authority)
            && let Some(token) = &self.token
        {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let unreadable = |why: String| HubMetadata {
            note: Some(format!(
                "the metadata of {repo} could not be read ({why}): its licence is unknown"
            )),
            ..HubMetadata::default()
        };
        let response = match request.call() {
            Ok(response) => response,
            Err(err) => return unreadable(err.to_string()),
        };
        let status = response.status().as_u16();
        if status != 200 {
            return unreadable(format!("HTTP {status}"));
        }
        let mut body = Vec::new();
        if let Err(err) = response
            .into_body()
            .into_reader()
            .take(MAX_METADATA_BYTES)
            .read_to_end(&mut body)
        {
            return unreadable(err.to_string());
        }
        let Ok(value) = serde_json::from_slice::<Value>(&body) else {
            return unreadable("not JSON".to_string());
        };
        HubMetadata::from_json(&value)
    }

    /// Downloads `url` (the file `name` of `repo`) into `dir/name`, following redirects
    /// itself so the stated hash of the first answer is seen and the token goes to the
    /// endpoint's host only.
    #[allow(clippy::too_many_arguments)]
    fn fetch(
        &self,
        url: &str,
        repo: &str,
        revision: &str,
        name: &str,
        dir: &Path,
        optional: bool,
        out: &mut dyn Write,
        live: bool,
    ) -> Result<Option<Fetched>, String> {
        let mut url = url.to_string();
        let mut stated: Option<Stated> = None;
        let mut expected_size: Option<u64> = None;
        let mut commit: Option<String> = None;
        for _ in 0..10 {
            let mut request = self.agent.get(&url);
            let same_host = origin_of(&url).is_some_and(|(_, a)| a == self.endpoint_authority);
            if same_host && let Some(token) = &self.token {
                request = request.header("Authorization", format!("Bearer {token}"));
            }
            let response = request
                .call()
                .map_err(|err| format!("cannot fetch {name} of {repo}: {err}"))?;
            let status = response.status().as_u16();
            if stated.is_none() {
                stated = header(&response, "x-linked-etag").and_then(|v| etag_hash(&v));
            }
            if expected_size.is_none() {
                expected_size =
                    header(&response, "x-linked-size").and_then(|v| v.trim().parse().ok());
            }
            if commit.is_none() {
                commit = header(&response, "x-repo-commit");
            }
            match status {
                200 => {
                    if stated.is_none() {
                        stated = header(&response, "etag").and_then(|v| etag_hash(&v));
                    }
                    if expected_size.is_none() {
                        expected_size =
                            header(&response, "content-length").and_then(|v| v.trim().parse().ok());
                    }
                    let (sha256, size) = stream(
                        response,
                        name,
                        dir,
                        expected_size,
                        stated.as_ref(),
                        out,
                        live,
                    )?;
                    return Ok(Some(Fetched {
                        name: name.to_string(),
                        size,
                        sha256,
                        commit,
                    }));
                }
                301 | 302 | 303 | 307 | 308 => {
                    let location = header(&response, "location").ok_or_else(|| {
                        format!("{name} of {repo}: a redirect without a Location")
                    })?;
                    url = resolve_location(&url, &location)
                        .ok_or_else(|| format!("{name} of {repo}: cannot follow the redirect"))?;
                }
                401 | 403 => {
                    return Err(format!(
                        "{repo} refused the download of {name} (HTTP {status}): the repository \
                         does not exist (Hugging Face answers 401 then), or it is gated or \
                         private and needs a Hugging Face token in HF_TOKEN (and the licence \
                         accepted on its page)"
                    ));
                }
                404 if optional => return Ok(None),
                404 => {
                    return Err(format!(
                        "{repo} at revision {revision} has no {name} (HTTP 404): check the \
                         repository name; a Model2Vec model holds config.json, tokenizer.json \
                         and model.safetensors at its root"
                    ));
                }
                _ => {
                    return Err(format!(
                        "the download of {name} of {repo} failed: HTTP {status}"
                    ));
                }
            }
        }
        Err(format!("{name} of {repo}: too many redirects"))
    }
}

/// Streams the body into `dir/name.part`, checks size and hash, renames it to `dir/name`.
fn stream(
    response: ureq::http::Response<ureq::Body>,
    name: &str,
    dir: &Path,
    expected_size: Option<u64>,
    stated: Option<&Stated>,
    out: &mut dyn Write,
    live: bool,
) -> Result<(String, u64), String> {
    let part = dir.join(format!("{name}.part"));
    let mut file = fs::File::create(&part)
        .map_err(|err| format!("cannot create {}: {err}", part.display()))?;
    let mut reader = response.into_body().into_reader();
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    // A git blob id needs the size up front: checked only when the server states it.
    let mut git = match (stated, expected_size) {
        (Some(Stated::GitSha1(_)), Some(size)) => {
            let mut context = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
            context.update(format!("blob {size}\0").as_bytes());
            Some(context)
        }
        _ => None,
    };
    let mut buffer = vec![0u8; 256 * 1024];
    let mut total: u64 = 0;
    let limit = expected_size.unwrap_or(MAX_FILE_BYTES);
    let progress = Progress::new(name, expected_size, live);
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|err| format!("the download of {name} broke off: {err}"))?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > limit {
            return Err(format!("{name} is larger than {}", human_bytes(limit)));
        }
        digest.update(&buffer[..read]);
        if let Some(git) = git.as_mut() {
            git.update(&buffer[..read]);
        }
        file.write_all(&buffer[..read])
            .map_err(|err| format!("cannot write {}: {err}", part.display()))?;
        progress.update(total, out);
    }
    file.sync_all()
        .map_err(|err| format!("cannot write {}: {err}", part.display()))?;
    drop(file);
    progress.finish(total, out);
    if let Some(expected) = expected_size
        && total != expected
    {
        return Err(format!(
            "the download of {name} broke off: {total} of {expected} bytes"
        ));
    }
    let sha256 = hex(digest.finish().as_ref());
    let mismatch = match stated {
        Some(Stated::Sha256(expected)) if *expected != sha256 => {
            Some(format!("SHA-256 {sha256}, the server states {expected}"))
        }
        Some(Stated::GitSha1(expected)) => git
            .map(|git| hex(git.finish().as_ref()))
            .filter(|actual| actual != expected)
            .map(|actual| format!("git blob id {actual}, the server states {expected}")),
        _ => None,
    };
    if let Some(mismatch) = mismatch {
        return Err(format!(
            "{name}: the download is not the file the server describes ({mismatch}); nothing \
             was kept"
        ));
    }
    fs::rename(&part, dir.join(name))
        .map_err(|err| format!("cannot rename {}: {err}", part.display()))?;
    Ok((sha256, total))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The progress line of one file: rewritten in place on a terminal, else one line at the end.
struct Progress {
    name: String,
    expected: Option<u64>,
    live: bool,
    last: std::cell::Cell<Instant>,
}

impl Progress {
    fn new(name: &str, expected: Option<u64>, live: bool) -> Self {
        Progress {
            name: name.to_string(),
            expected,
            live,
            last: std::cell::Cell::new(Instant::now()),
        }
    }

    fn text(&self, done: u64) -> String {
        match self.expected {
            Some(total) if total > 0 => format!(
                "    {}: {} / {} ({}%)",
                self.name,
                human_bytes(done),
                human_bytes(total),
                done.saturating_mul(100) / total
            ),
            _ => format!("    {}: {}", self.name, human_bytes(done)),
        }
    }

    fn update(&self, done: u64, out: &mut dyn Write) {
        if self.live && self.last.get().elapsed() >= Duration::from_millis(200) {
            self.last.set(Instant::now());
            let _ = write!(out, "\r{}\x1b[K", self.text(done));
            let _ = out.flush();
        }
    }

    fn finish(&self, done: u64, out: &mut dyn Write) {
        if self.live {
            let _ = writeln!(out, "\r{}\x1b[K", self.text(done));
        } else {
            let _ = writeln!(out, "{}", self.text(done));
        }
        let _ = out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_names() {
        for ok in ["minishlab/potion-base-8M", "bert-base-uncased", "a.b/c_d-e"] {
            assert!(is_repo_name(ok), "{ok}");
        }
        for bad in [
            "",
            "/",
            "a/",
            "/a",
            "a/b/c",
            "../x",
            "a/..",
            "a b/c",
            "a/.hidden",
        ] {
            assert!(!is_repo_name(bad), "{bad}");
        }
    }

    #[test]
    fn redirects_resolve() {
        let base = "https://huggingface.co/a/b/resolve/main/model.safetensors";
        assert_eq!(
            resolve_location(base, "https://cdn.example/x?y=1").as_deref(),
            Some("https://cdn.example/x?y=1")
        );
        assert_eq!(
            resolve_location(base, "/api/resolve-cache/models/a/b/c/config.json").as_deref(),
            Some("https://huggingface.co/api/resolve-cache/models/a/b/c/config.json")
        );
        assert_eq!(
            resolve_location(base, "other.json").as_deref(),
            Some("https://huggingface.co/a/b/resolve/main/other.json")
        );
        assert_eq!(
            resolve_location("http://127.0.0.1:9/x", "//cdn/y").as_deref(),
            Some("http://cdn/y")
        );
        assert_eq!(
            origin_of("http://127.0.0.1:8080/a/b"),
            Some(("http://127.0.0.1:8080", "127.0.0.1:8080"))
        );
    }

    #[test]
    fn etags() {
        let hex = "a".repeat(64);
        assert_eq!(
            etag_hash(&format!("\"{hex}\"")),
            Some(Stated::Sha256(hex.clone()))
        );
        assert_eq!(
            etag_hash(&format!("W/\"{hex}\"")),
            Some(Stated::Sha256(hex))
        );
        let sha1 = "b".repeat(40);
        assert_eq!(
            etag_hash(&format!("\"{sha1}\"")),
            Some(Stated::GitSha1(sha1))
        );
        assert_eq!(etag_hash("\"0123abcd\""), None);
        assert_eq!(etag_hash(&format!("\"{}\"", "z".repeat(64))), None);
    }

    #[test]
    fn sizes() {
        assert_eq!(human_bytes(10), "10 B");
        assert_eq!(human_bytes(2048), "2.0 KiB");
        assert_eq!(human_bytes(30 * 1024 * 1024), "30.0 MiB");
    }

    #[test]
    fn licence_warnings_name_the_problems() {
        assert!(licence_warnings("a/b", "mit", false).is_empty());
        assert!(licence_warnings("a/b", "unknown", false)[0].contains("unknown"));
        assert!(licence_warnings("a/b", "cc-by-nc-4.0", false)[0].contains("NON-COMMERCIAL"));
        assert!(licence_warnings("a/b", "mit", true)[0].contains("GATED"));
        let files = licence_files(&["LICENSE".into(), "sub/LICENSE".into(), "NOTICE.txt".into()]);
        assert_eq!(
            files,
            vec![
                ("LICENSE".to_string(), Role::License),
                ("NOTICE.txt".to_string(), Role::Notice)
            ]
        );
    }

    #[test]
    fn the_source_follows_the_environment() {
        let source = Source::from_env(&|name| match name {
            ENDPOINT_ENV => Some("http://127.0.0.1:1/".into()),
            "HF_TOKEN" => Some("t".into()),
            _ => None,
        });
        assert_eq!(source.endpoint, "http://127.0.0.1:1");
        assert_eq!(source.token.as_deref(), Some("t"));
        assert_eq!(Source::from_env(&|_| None).endpoint, DEFAULT_ENDPOINT);
    }
}
