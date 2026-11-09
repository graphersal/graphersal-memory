//! The graph engine this binary is built on.
//!
//! graphersal-memory is a consumer of the public graphersal API. An engine gap found here is
//! fixed in the graphersal repository, never worked around with a local copy of engine code.

/// The graphersal git revision pinned in the workspace `Cargo.toml` (first 8 hex digits), as a
/// literal for `concat!`.
macro_rules! engine_rev_short {
    () => {
        "063be87d"
    };
}
pub(crate) use engine_rev_short;

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

    /// The revision shown by `--version` is the one the workspace pins.
    #[test]
    fn version_revision_matches_the_pin() {
        let manifest = include_str!("../../../Cargo.toml");
        let pin = concat!("rev = \"", engine_rev_short!());
        assert_eq!(
            manifest.matches(pin).count(),
            2,
            "both engine crates pin the same rev"
        );
    }
}
