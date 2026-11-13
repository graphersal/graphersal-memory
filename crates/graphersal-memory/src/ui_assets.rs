//! The browser UI's own files, built into the binary: the `memory` profile
//! (`crates/graphersal-memory/ui/profile.json`, delivered as `uiProfile` of the relay
//! context's `GET /api/info`) and its modules, served by `graphersal-memory ui` under
//! [`FILES_PREFIX`] before the embedded playground (`crate::ui`). `ui/tests/` stays out: the
//! `node --test` tests of the pure modules.

use serde_json::Value;

/// The path the profile's files are served under (the profile's `files`).
pub const FILES_PREFIX: &str = "/memory-ui/";

/// The profile object.
const PROFILE: &str = include_str!("../ui/profile.json");

/// The files under [`FILES_PREFIX`]: name and bytes.
const FILES: [(&str, &[u8]); 13] = [
    ("boot.js", include_bytes!("../ui/boot.js")),
    ("memory-ui.css", include_bytes!("../ui/memory-ui.css")),
    ("model.js", include_bytes!("../ui/model.js")),
    ("api-ui.js", include_bytes!("../ui/api-ui.js")),
    ("shared.js", include_bytes!("../ui/shared.js")),
    ("overview.js", include_bytes!("../ui/overview.js")),
    ("memory.js", include_bytes!("../ui/memory.js")),
    ("graph-model.js", include_bytes!("../ui/graph-model.js")),
    ("graph.js", include_bytes!("../ui/graph.js")),
    ("always.js", include_bytes!("../ui/always.js")),
    ("conflicts.js", include_bytes!("../ui/conflicts.js")),
    ("sessions.js", include_bytes!("../ui/sessions.js")),
    ("profile.json", PROFILE.as_bytes()),
];

/// The profile object (the engine's playground validates it), with this binary's version as
/// its `version`: the page's header shows graphersal-memory's own version (the engine's moves
/// into the header's tooltip).
pub fn profile() -> Result<Value, String> {
    let mut profile: Value = serde_json::from_str(PROFILE)
        .map_err(|err| format!("the built-in UI profile is invalid: {err}"))?;
    if let Some(object) = profile.as_object_mut() {
        object.insert(
            "version".to_string(),
            Value::String(env!("CARGO_PKG_VERSION").to_string()),
        );
    }
    Ok(profile)
}

/// The bytes and content type of the profile file `name` (the path after [`FILES_PREFIX`]).
pub fn file(name: &str) -> Option<(&'static [u8], &'static str)> {
    FILES
        .iter()
        .find(|(file, _)| *file == name)
        .map(|(file, bytes)| (*bytes, content_type(file)))
}

/// The content type of a file name by its extension.
pub fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "txt" | "md" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_names_its_files_and_every_module_is_built_in() {
        let profile = profile().unwrap();
        assert_eq!(profile["name"], "memory");
        assert_eq!(profile["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(profile["files"], FILES_PREFIX);
        let tabs = profile["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 6);
        for tab in tabs {
            let module = tab["module"].as_str().unwrap();
            assert!(file(module).is_some(), "{module} is not built in");
            assert!(
                tab.get("builtin").is_none(),
                "allow-list mode has no workspace"
            );
        }
        for label in ["memory", "session", "fact"] {
            assert!(profile["cards"][label].is_object(), "{label}");
        }
        assert_eq!(
            file("model.js").unwrap().1,
            "text/javascript; charset=utf-8"
        );
        assert!(file("../x.js").is_none());
        assert!(file("tests/model.test.mjs").is_none());
    }
}
