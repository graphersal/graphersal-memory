//! The graph engine this binary is built on.
//!
//! graphersal-memory is a consumer of the public graphersal API. An engine gap found here is
//! fixed in the graphersal repository, never worked around with a local copy of engine code.

/// The graphersal version pinned in the workspace `Cargo.toml` (every engine crate pins the same
/// exact version), as a literal for `concat!`.
macro_rules! engine_version {
    () => {
        "0.6.2"
    };
}
pub(crate) use engine_version;

#[cfg(test)]
mod tests {
    use graphersal::prelude::*;

    /// Exercises the pinned engine crates, so a pin that no longer builds fails here.
    #[test]
    fn pinned_engine_crates_resolve() {
        let graph = TraversalGraph::new();
        assert_eq!(graph.vertex_count(), 0);
        const { assert!(graphersal_mcp::MIN_ANSWER_CHARS > 0) };
    }

    /// The version shown by `--version` is the one the workspace pins for every engine crate,
    /// and the one the lock file resolved.
    #[test]
    fn version_matches_the_pin() {
        let manifest = include_str!("../../../../Cargo.toml");
        let pin = concat!("\"=", engine_version!(), "\"");
        assert_eq!(
            manifest.matches(pin).count(),
            4,
            "every engine crate pins the same exact version"
        );
        let lock = include_str!("../../../../Cargo.lock");
        for name in [
            "graphersal",
            "graphersal-mcp",
            "graphersal-serve",
            "graphersal-session",
        ] {
            let entry = format!("name = \"{name}\"\nversion = \"{}\"", engine_version!());
            assert!(lock.contains(&entry), "Cargo.lock resolves {name}");
        }
    }
}
