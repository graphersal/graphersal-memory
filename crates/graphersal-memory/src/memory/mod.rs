//! What the memory holds and how it gets there: the file-memory import and the export,
//! project identity, the start context, instruction files, past sessions (indexing, the shim's
//! session start and end, transcripts, summaries) and the hooks.

pub mod export;
pub mod hook;
pub mod import;
pub mod instructions;
pub mod projects;
pub mod session_sync;
pub mod sessions;
pub mod start_context;
pub mod summaries;
pub mod transcript;
