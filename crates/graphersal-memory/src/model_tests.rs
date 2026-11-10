//! The embedding model: the download from a local fake Hugging Face server (never the network),
//! the install step that uses it, and the store switching its vector rules to the model and
//! back. The model is a TINY synthetic Model2Vec model (a word-level tokenizer of four tokens,
//! a 4 x 3 matrix), as the engine's own tests build it.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::install::{self, ClaudePaths, InstallOptions, Setup};
use crate::mcp::{self, StoreRuntime};
use crate::mcp_tests::TempDir;
use crate::{download, models, registry, store};

const VOCAB: [&str; 4] = ["[UNK]", "graph", "vector", "model"];

fn tokenizer_json() -> String {
    let entries: Vec<String> = VOCAB
        .iter()
        .enumerate()
        .map(|(id, token)| format!("{token:?}: {id}"))
        .collect();
    format!(
        r#"{{"version": "1.0", "truncation": null, "padding": null, "added_tokens": [],
            "normalizer": {{"type": "Lowercase"}}, "pre_tokenizer": {{"type": "Whitespace"}},
            "post_processor": null, "decoder": null,
            "model": {{"type": "WordLevel", "vocab": {{{}}}, "unk_token": "[UNK]"}}}}"#,
        entries.join(", ")
    )
}

fn safetensors() -> Vec<u8> {
    let data: Vec<u8> = (0..VOCAB.len() * 3)
        .flat_map(|i| (1.0f32 + i as f32).to_le_bytes())
        .collect();
    let mut header = format!(
        r#"{{"embeddings": {{"dtype": "F32", "shape": [{}, 3], "data_offsets": [0, {}]}}}}"#,
        VOCAB.len(),
        data.len()
    );
    while header.len() % 8 != 0 {
        header.push(' ');
    }
    let mut out = (header.len() as u64).to_le_bytes().to_vec();
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(&data);
    out
}

fn sha256(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A fake Hugging Face endpoint serving the tiny model as `acme/tiny`, and as `acme/broken`
/// with a wrong stated hash of its weights. Returns its URL and the paths it was asked for.
fn fake_hub() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
            seen.lock().unwrap().push(path.clone());
            let file = |name: &str| -> Option<Vec<u8>> {
                Some(match name {
                    "config.json" => br#"{"normalize": true}"#.to_vec(),
                    "tokenizer.json" => tokenizer_json().into_bytes(),
                    "model.safetensors" => safetensors(),
                    "README.md" => b"---\nlicense: mit\n---\n# tiny\n".to_vec(),
                    _ => return None,
                })
            };
            let (status, headers, body): (&str, String, Vec<u8>) =
                if let Some(rest) = path.strip_prefix("/api/models/") {
                    let repo = rest.split("/revision").next().unwrap_or(rest);
                    let body = json!({"cardData": {"license": "mit"}, "gated": false,
                    "siblings": [{"rfilename": "config.json"}, {"rfilename": "LICENSE"}],
                    "id": repo})
                    .to_string()
                    .into_bytes();
                    ("200 OK", String::new(), body)
                } else if let Some((repo, name)) =
                    path.trim_start_matches('/').split_once("/resolve/main/")
                {
                    match (name, file(name)) {
                        ("LICENSE", _) => ("200 OK", String::new(), b"MIT License\n".to_vec()),
                        (_, Some(bytes)) => {
                            let stated = if repo == "acme/broken" && name == "model.safetensors" {
                                "0".repeat(64)
                            } else {
                                sha256(&bytes)
                            };
                            (
                                "200 OK",
                                format!(
                                    "X-Linked-Etag: \"{stated}\"\r\nX-Repo-Commit: {}\r\n",
                                    "c".repeat(40)
                                ),
                                bytes,
                            )
                        }
                        _ => ("404 Not Found", String::new(), Vec::new()),
                    }
                } else {
                    ("404 Not Found", String::new(), Vec::new())
                };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });
    (url, asked)
}

#[test]
fn the_download_checks_hashes_records_the_licence_and_leaves_nothing_on_failure() {
    let (url, asked) = fake_hub();
    let data = TempDir::new("dl");
    let source = download::Source {
        endpoint: url.clone(),
        token: None,
    };
    let mut out = Vec::new();
    let done = download::download(
        "acme/tiny",
        &models::cache_dir(data.path()),
        &source,
        &mut out,
        false,
    )
    .unwrap();
    assert_eq!(done.license, "mit");
    assert!(
        done.identity.starts_with("model2vec/acme/tiny@"),
        "{}",
        done.identity
    );
    let cached = models::cached(data.path(), "acme/tiny").unwrap();
    assert_eq!(cached.license, "mit");
    assert!(models::check(data.path(), "acme/tiny").is_ok());
    assert!(models::describe(data.path(), "acme/tiny").contains("downloaded"));
    assert!(
        models::model_dir(data.path(), "acme/tiny")
            .join("LICENSE")
            .exists(),
        "the licence file travels with the model"
    );
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("model.safetensors"), "{text}");
    assert!(
        asked
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.starts_with("/api/models/acme/tiny"))
    );

    let mut out = Vec::new();
    let err = download::download(
        "acme/broken",
        &models::cache_dir(data.path()),
        &source,
        &mut out,
        false,
    )
    .unwrap_err();
    assert!(err.contains("not the file the server describes"), "{err}");
    assert!(models::cached(data.path(), "acme/broken").is_none());
    let leftovers: Vec<_> = std::fs::read_dir(models::cache_dir(data.path()).join("acme"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        leftovers,
        vec!["tiny".to_string()],
        "nothing of the failed download stays"
    );
}

fn setup_with(home: &Path, data: &Path, endpoint: &str) -> Setup {
    let paths =
        ClaudePaths::resolve(&|name| (name == "HOME").then(|| home.display().to_string())).unwrap();
    Setup {
        paths,
        opencode: crate::opencode::OpenCodePaths::of_home(home),
        codex: crate::codex::CodexPaths::of_home(home),
        cursor: crate::cursor::CursorPaths::of_home(home, data),
        search_path: Some(std::ffi::OsString::new()),
        data_dir: data.to_path_buf(),
        run_dir: std::env::temp_dir().join(format!("gmm{}", std::process::id())),
        exe: std::path::PathBuf::from("/opt/gm/bin/graphersal-memory"),
        dir_args: vec!["--data-dir".into(), data.display().to_string()],
        hooks: crate::install::HookForm::posix(),
        source: download::Source {
            endpoint: endpoint.to_string(),
            token: None,
        },
    }
}

fn install_with_model(setup: &Setup, model: &str) -> (u8, String) {
    let mut connect = |_: &str| -> Result<Box<dyn crate::import::Caller>, String> {
        Err("no import in this test".to_string())
    };
    let mut out = Vec::new();
    let options = InstallOptions {
        yes: true,
        model: Some(model.to_string()),
        ..InstallOptions::default()
    };
    let code = install::install(setup, &options, &mut connect, &mut &b""[..], &mut out).unwrap();
    (code, String::from_utf8(out).unwrap())
}

#[test]
fn install_downloads_the_model_and_a_failed_download_keeps_the_builtin_embedder() {
    let (url, _) = fake_hub();
    let home = TempDir::new("mi-home");
    let data = TempDir::new("mi-data");
    let setup = setup_with(home.path(), data.path(), &url);
    let (code, out) = install_with_model(&setup, "acme/broken");
    assert_eq!(code, 0, "a failed download does not fail install: {out}");
    assert!(out.contains("the download of acme/broken failed"), "{out}");
    assert!(
        out.contains("built-in embedder hash-ngram/1 stays"),
        "{out}"
    );
    assert_eq!(
        crate::config::Config::load(data.path()).unwrap().model,
        None
    );

    let (code, out) = install_with_model(&setup, "acme/tiny");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("downloaded acme/tiny"), "{out}");
    assert!(out.contains("licence mit"), "{out}");
    assert_eq!(
        crate::config::Config::load(data.path())
            .unwrap()
            .model
            .as_deref(),
        Some("acme/tiny")
    );
    let status = install::status_lines(&setup).join("\n");
    assert!(status.contains("embedding model: acme/tiny ("), "{status}");
    let doctor = install::doctor(&setup);
    assert!(
        doctor
            .iter()
            .any(|f| !f.problem && f.text.contains("acme/tiny is present and loads")),
        "{doctor:?}"
    );
    // Again: downloaded already, nothing fetched.
    let (code, out) = install_with_model(&setup, "acme/tiny");
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("Downloading"), "{out}");
    // A model removed from the cache is a doctor problem.
    std::fs::remove_file(models::model_dir(data.path(), "acme/tiny").join("model.safetensors"))
        .unwrap();
    assert!(
        install::doctor(&setup)
            .iter()
            .any(|f| f.problem && f.text.contains("acme/tiny is not downloaded"))
    );
}

fn remember(runtime: &StoreRuntime, slug: &str, summary: &str) {
    runtime
        .call(
            "remember",
            json!({"slug": slug, "kind": "project", "summary": summary, "body": summary,
                   "why": "", "how_to_apply": "", "title": "", "hook": "", "links": [],
                   "about": [], "scope": "project", "project": "/w"}),
        )
        .unwrap();
}

fn memories(runtime: &StoreRuntime, text: &str) -> Value {
    runtime
        .call(
            "memories",
            json!({"text": text, "k": 5, "kind": "", "project": "/w"}),
        )
        .unwrap()
}

#[test]
fn the_store_switches_its_vector_rules_to_the_model_and_back() {
    let (url, _) = fake_hub();
    let data = TempDir::new("sw-data");
    let source = download::Source {
        endpoint: url,
        token: None,
    };
    download::download(
        "acme/tiny",
        &models::cache_dir(data.path()),
        &source,
        &mut Vec::new(),
        false,
    )
    .unwrap();
    let dir = store::default_store_dir(data.path());
    let runtime =
        StoreRuntime::open(registry::DEFAULT_STORE, &dir, Arc::new(mcp::endpoint())).unwrap();
    remember(&runtime, "graph-first", "graph before grep");
    assert_eq!(runtime.embedder().unwrap(), models::BUILTIN);
    assert!(runtime.switch_embedder(data.path(), "acme/tiny").unwrap());
    let embedder = runtime.embedder().unwrap();
    assert!(embedder.starts_with("model2vec/acme/tiny@"), "{embedder}");
    assert!(
        !runtime.switch_embedder(data.path(), "acme/tiny").unwrap(),
        "no-op the second time"
    );
    // The memories written before and after are found with the model.
    remember(&runtime, "vector-model", "vector model notes");
    let found = memories(&runtime, "vector model");
    assert!(found.to_string().contains("vector-model"), "{found}");
    // A store reopened keeps the model (the catalog keeps its embedder too).
    runtime.close().unwrap();
    let runtime =
        StoreRuntime::open(registry::DEFAULT_STORE, &dir, Arc::new(mcp::endpoint())).unwrap();
    assert!(
        runtime
            .embedder()
            .unwrap()
            .starts_with("model2vec/acme/tiny@")
    );
    assert!(
        runtime
            .switch_embedder(data.path(), models::BUILTIN)
            .unwrap()
    );
    assert_eq!(runtime.embedder().unwrap(), models::BUILTIN);
    let found = memories(&runtime, "graph");
    assert!(found.to_string().contains("graph-first"), "{found}");
    // A model that is not downloaded is refused; the store keeps its embedder.
    let err = runtime
        .switch_embedder(data.path(), "acme/missing")
        .unwrap_err();
    assert!(err.contains("not downloaded"), "{err}");
    assert_eq!(runtime.embedder().unwrap(), models::BUILTIN);
    runtime.close().unwrap();
}
