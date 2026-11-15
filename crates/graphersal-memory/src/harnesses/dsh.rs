//! DeepSeek Harness (dsh; not CodeWhale, which has a module of its own); verified against the
//! source of tag `dsh-v0.2.2-rc.3` (38a6a5c), each fact names its file. For now only how its
//! sessions are read ([`sessions`]); its paths, its `Harness` and its registry lines follow.
//!
//! - **Sessions root**: `$DSH_HOME/sessions`, `$DSH_HOME` else `<home>/.dsh`
//!   (`packages/bundle/base/cordis.patch.yml:138-141`).
//! - **Layout**: `<root>/<projectKey(cwd)>/<encodeSegment(id)>/session.v4.jsonl.zstd`, or
//!   `session.v4.jsonl` uncompressed (the `sdk-minimal` profile, `compression: none`, same root)
//!   (`packages/session/session-persistence-jsonl/src/format.ts:199-304`,
//!   `packages/session/session-format/src/filename.ts:14-17`); a session without a working
//!   directory sits under `_no-cwd/`. The project key is lossy (runs of `/ \ :` become `-`,
//!   other characters `~XXXX`, cut to 251, `--<slug>--`): it is never decoded, the project is
//!   the header's `cwd`. Older generations (`session.jsonl`, `session.v1.jsonl` ..
//!   `session.v3.jsonl[.zstd]`) may sit beside the v4 file and are not read.
//! - **Format**: JSON Lines; line 1 the header `{"type": "session", "version": 4, "id":
//!   "session-<uuid>", "createdAt": <ms>, "cwd"?, "parentSession"?, "isSeeded", "origin"?:
//!   "subagent", "delegationDepth", "agentPreset"?}`, then one event per line `{type, seq, time
//!   (ms), data, surfaceOp?, sourceEventSeqs?, ignorable?}`: `turn/start`/`turn/end` (`turn`),
//!   `user/message` (no turn; `source.kind` `user` for the human, other kinds injected),
//!   `assistant/message` (`turn`, `message.content` parts), `tool/call` (`turn`, `name`,
//!   `arguments` as JSON text; MCP tools as `mcp__<server>__<tool>`), `tool/result`. Version 4
//!   may gain event types without a version bump. There is no session end event.
//! - **Compression**: the `.zstd` file is a concatenation of independent standard zstd frames
//!   (one with the header line, one per append batch), each with a content checksum, no
//!   dictionary, no skippable frames; the last frame can be torn by a crash or a concurrent
//!   write.

pub mod sessions;
