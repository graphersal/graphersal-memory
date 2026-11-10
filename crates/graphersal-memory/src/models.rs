//! The embedding models `install` offers, the model cache in the data directory
//! (`<data dir>/models/<owner>/<name>/`, filled by `crate::download`), and what `status` and
//! `doctor` say about the configured model.
//!
//! The model makes the vectors the memory search compares (`emb` of every memory, session,
//! turn and fact, a vector rule of the schema). The built-in `hash-ngram/1` needs no download
//! but compares shared character n-grams only: it finds the same words and spellings, not the
//! same meaning. A Model2Vec model (a static embedding model, a few hundred MB, runs on the CPU
//! in microseconds per text) finds related wording too.

use std::path::{Path, PathBuf};

use crate::language::Language;

/// The built-in embedder's name (the settings value of "no model").
pub const BUILTIN: &str = "hash-ngram/1";

/// The model offered for English memories.
pub const ENGLISH_MODEL: &str = "minishlab/potion-retrieval-32M";

/// The model offered for memories in any language.
pub const MULTILINGUAL_MODEL: &str = "minishlab/potion-multilingual-128M";

/// A model the wizard offers. Sizes and licences come from the Hugging Face metadata of the
/// repositories (read 2026-11-10: the files a download fetches, `config.json` +
/// `tokenizer.json` + `model.safetensors`; licence `cardData.license`); they are approximate
/// and may change upstream, the download states the real figures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offered {
    /// The Hugging Face repository.
    pub repo: &'static str,
    /// The languages, as the wizard shows them.
    pub languages: &'static str,
    /// The approximate download size, as the wizard shows it.
    pub size: &'static str,
    /// The licence (SPDX), as the repository states it.
    pub license: &'static str,
}

/// The downloadable models the wizard offers, English first.
pub const OFFERED: [Offered; 2] = [
    Offered {
        repo: ENGLISH_MODEL,
        languages: "English",
        size: "~130 MB",
        license: "MIT",
    },
    Offered {
        repo: MULTILINGUAL_MODEL,
        languages: "100+ languages",
        size: "~530 MB",
        license: "MIT",
    },
];

/// The model the wizard proposes for a language.
pub fn default_for(language: Language) -> &'static str {
    match language {
        Language::English => ENGLISH_MODEL,
        Language::Any => MULTILINGUAL_MODEL,
    }
}

/// The offered model of `repo`, if it is one.
pub fn offered(repo: &str) -> Option<&'static Offered> {
    OFFERED.iter().find(|model| model.repo == repo)
}

/// The short name of a repository (`potion-retrieval-32M`).
pub fn short_name(repo: &str) -> &str {
    repo.rsplit('/').next().unwrap_or(repo)
}

/// The model cache of a data directory.
pub fn cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

/// Where the model `repo` is cached.
pub fn model_dir(data_dir: &Path, repo: &str) -> PathBuf {
    cache_dir(data_dir).join(repo)
}

/// What the cache holds of `repo`: its manifest's licence and the bytes of its model files,
/// when it is downloaded completely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached {
    /// The licence the manifest records.
    pub license: String,
    /// The bytes of the three model files.
    pub bytes: u64,
}

/// The cached model `repo`, or `None` when it is not (completely) downloaded.
pub fn cached(data_dir: &Path, repo: &str) -> Option<Cached> {
    let dir = model_dir(data_dir, repo);
    let manifest = std::fs::read(dir.join(graphersal::persist::ModelCache::MANIFEST_FILE)).ok()?;
    let manifest: serde_json::Value = serde_json::from_slice(&manifest).ok()?;
    let mut bytes = 0;
    for file in crate::download::MODEL_FILES {
        bytes += std::fs::metadata(dir.join(file)).ok()?.len();
    }
    Some(Cached {
        license: manifest["license"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        bytes,
    })
}

/// One line about the model `model` (a repository or [`BUILTIN`]) for `status`.
pub fn describe(data_dir: &Path, model: &str) -> String {
    if model == BUILTIN {
        return format!("{BUILTIN} (built-in, no download; finds shared words, not meaning)");
    }
    match cached(data_dir, model) {
        Some(cached) => format!(
            "{model} ({}, {}, downloaded)",
            crate::download::human_bytes(cached.bytes),
            cached.license
        ),
        None => {
            format!("{model} (NOT downloaded: run `graphersal-memory install --model {model}`)")
        }
    }
}

/// Whether the cached model `repo` loads as a Model2Vec model (`doctor`): `Err` says why not.
pub fn check(data_dir: &Path, repo: &str) -> Result<String, String> {
    let dir = model_dir(data_dir, repo);
    if cached(data_dir, repo).is_none() {
        return Err(format!(
            "{repo} is not downloaded ({} is missing or incomplete)",
            dir.display()
        ));
    }
    graphersal::persist::ModelCache::check(&dir)
        .map_err(|err| format!("{repo} does not load: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_picks_the_default_model() {
        assert_eq!(default_for(Language::English), ENGLISH_MODEL);
        assert_eq!(default_for(Language::Any), MULTILINGUAL_MODEL);
        assert_eq!(short_name(ENGLISH_MODEL), "potion-retrieval-32M");
        assert!(offered(MULTILINGUAL_MODEL).is_some());
        assert!(offered("x/y").is_none());
        for model in OFFERED {
            assert!(crate::download::is_repo_name(model.repo));
        }
    }

    #[test]
    fn a_missing_model_is_described_and_fails_the_check() {
        let data = crate::mcp_tests::TempDir::new("models");
        assert!(describe(data.path(), BUILTIN).contains("built-in"));
        assert!(describe(data.path(), ENGLISH_MODEL).contains("NOT downloaded"));
        assert!(check(data.path(), ENGLISH_MODEL).is_err());
        assert_eq!(cached(data.path(), ENGLISH_MODEL), None);
    }
}
