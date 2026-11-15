//! DeepSeek Harness's sessions (`crate::harnesses::dsh` has the facts and their sources): one
//! directory per session, `<root>/<project key>/<session id>/`, holding `session.v4.jsonl.zstd`
//! (or `session.v4.jsonl` uncompressed), read into the [`Transcript`] the indexing writes (never
//! changed).
//!
//! - **Units**: the v4 file of each session directory, two levels under the root (`_no-cwd/`
//!   included); when both v4 files exist in one directory, the one modified last (on a tie the
//!   `.zstd` one, the default profile's). A directory with only older generations
//!   (`session.jsonl`, `session.v1.jsonl` .. `session.v3.jsonl[.zstd]`) has no unit.
//! - **Unit id**: `dsh-<uuid>` from the session directory's name (`session-<uuid>`, the
//!   `session-` taken off; another name kept whole after `dsh-`), so the graph's
//!   `session:dsh-<uuid>` cannot meet another harness's `session:<uuid>`. The source label is
//!   `dsh:<session directory>/<file name>` (every file has the same name).
//! - **zstd**: decoded frame by frame; a frame that does not decode to its end with a matching
//!   checksum (a torn last frame, being written or cut by a crash) ends the read with the
//!   frames before it. At most [`MAX_DECODED`] bytes are decoded (an error above).
//! - **Header**: a first line that is not `{"type": "session", ...}` or whose `version` is not
//!   4 makes the unit skipped ([`Skip`]): one log line and an empty transcript, which the
//!   indexing records as empty (not failed) until the file changes. A sub-agent's session
//!   (`origin: "subagent"`) and a session without a working directory (`_no-cwd/`: no project
//!   to file it under) have no turns.
//! - **Turns**: a human message (`user/message` with `source.kind` `user`) starts a turn; more
//!   human messages within the same dsh turn (the preceding `turn/start`) join it; injected
//!   messages (other source kinds) are not the user's words. `assistant/message` text parts are
//!   the answer, `tool/call` names the tools and, for our memory tools
//!   (`mcp__graphersal-memory__<function>`), the memories written; both go to the turn of their
//!   `turn` field, else the last one. Times are the events' `time` (milliseconds) as UTC
//!   timestamps; the start is the header's `createdAt`. Unknown event types, bad lines and a
//!   partial last line are skipped; the fingerprint covers the complete decoded lines.

use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::harnesses::opencode::sessions::timestamp;
use crate::memory::sessions::Unit;
use crate::memory::transcript::{CAP, Transcript, Turn, clip, is_windows_path, memory_writes};

/// The compressed v4 session file.
pub const V4_ZSTD: &str = "session.v4.jsonl.zstd";
/// The uncompressed v4 session file (profile `sdk-minimal`).
pub const V4_PLAIN: &str = "session.v4.jsonl";
/// The only header `version` read.
pub const VERSION: u64 = 4;
/// The most bytes a session file is decoded to.
pub const MAX_DECODED: usize = 512 * 1024 * 1024;
/// The most bytes of an uncompressed file read for its header line.
const MAX_HEADER: u64 = 1024 * 1024;
/// The zstd frame magic number, little-endian.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// Why a session file is skipped (its header is not one this reader knows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip(pub String);

/// The v4 session file of a session directory, when it has one (both: the one modified last,
/// the `.zstd` one on a tie).
pub fn session_file(dir: &Path) -> Option<PathBuf> {
    let modified = |path: &Path| {
        std::fs::metadata(path)
            .ok()
            .filter(|meta| meta.is_file())
            .map(|meta| meta.modified().ok())
    };
    let zstd = dir.join(V4_ZSTD);
    let plain = dir.join(V4_PLAIN);
    match (modified(&zstd), modified(&plain)) {
        (Some(z), Some(p)) => Some(if p > z { plain } else { zstd }),
        (Some(_), None) => Some(zstd),
        (None, Some(_)) => Some(plain),
        (None, None) => None,
    }
}

/// The session files under `root` (`<project key>/<session>/`), sorted.
pub fn scan(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(projects) = std::fs::read_dir(root) else {
        return out;
    };
    for project in projects.filter_map(Result::ok).map(|e| e.path()) {
        if !project.is_dir() {
            continue;
        }
        let Ok(sessions) = std::fs::read_dir(&project) else {
            continue;
        };
        out.extend(
            sessions
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|dir| dir.is_dir())
                .filter_map(|dir| session_file(&dir)),
        );
    }
    out.sort();
    out
}

/// The sessions of `root` as units (size and modification time).
pub fn units(root: &Path) -> Vec<Unit> {
    scan(root)
        .into_iter()
        .filter_map(|path| {
            let (size, modified) = crate::memory::sessions::stamp(&path)?;
            Some(Unit {
                path,
                size,
                modified,
            })
        })
        .collect()
}

/// The unit id of a session file: `dsh-<uuid>` from its directory's name.
pub fn unit_id(file: &Path) -> String {
    let name = file
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    format!("dsh-{}", name.strip_prefix("session-").unwrap_or(&name))
}

/// Whether `file` is a compressed session file.
fn is_zstd(file: &Path) -> bool {
    file.extension().is_some_and(|ext| ext == "zstd")
}

/// The frames of a zstd stream decoded, at most `max_frames` of them, ending at the first
/// frame that does not decode to its end with a matching checksum.
pub fn decode_frames(bytes: &[u8], max_frames: usize) -> Result<Vec<u8>, String> {
    if !bytes.is_empty() && !bytes.starts_with(&ZSTD_MAGIC) {
        return Err("not a zstd file".to_string());
    }
    // The decoder is not known to panic on bad input; should it, the frames before stay.
    let mut out = Vec::new();
    let mut rest = bytes;
    let mut decoder = ruzstd::decoding::FrameDecoder::new();
    let mut frames = 0;
    while !rest.is_empty() && frames < max_frames {
        let mut source = rest;
        let room = MAX_DECODED.saturating_sub(out.len());
        let frame = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            decode_frame(&mut decoder, &mut source, room)
        }))
        .unwrap_or(Ok(None));
        match frame? {
            Some(frame) => {
                out.extend_from_slice(&frame);
                rest = source;
                frames += 1;
            }
            None => break,
        }
    }
    Ok(out)
}

/// One frame from the start of `source` (advanced past it); `None` when it is torn or
/// corrupt, an error when it decodes to more than `room` bytes.
fn decode_frame(
    decoder: &mut ruzstd::decoding::FrameDecoder,
    source: &mut &[u8],
    room: usize,
) -> Result<Option<Vec<u8>>, String> {
    use ruzstd::decoding::BlockDecodingStrategy;
    if decoder.reset(&mut *source).is_err() {
        return Ok(None);
    }
    let mut frame = Vec::new();
    loop {
        let Ok(done) =
            decoder.decode_blocks(&mut *source, BlockDecodingStrategy::UptoBytes(1 << 20))
        else {
            return Ok(None);
        };
        if let Some(chunk) = decoder.collect() {
            frame.extend_from_slice(&chunk);
        }
        if frame.len() > room {
            return Err(format!(
                "decodes to more than {} MiB",
                MAX_DECODED / (1024 * 1024)
            ));
        }
        if done {
            break;
        }
    }
    if let (Some(stored), Some(computed)) = (
        decoder.get_checksum_from_data(),
        decoder.get_calculated_checksum(),
    ) && stored != computed
    {
        return Ok(None);
    }
    Ok(Some(frame))
}

/// The decoded JSON Lines of a session file.
pub fn load(file: &Path) -> Result<Vec<u8>, String> {
    let bytes =
        std::fs::read(file).map_err(|err| format!("cannot read {}: {err}", file.display()))?;
    if is_zstd(file) {
        decode_frames(&bytes, usize::MAX).map_err(|err| format!("{}: {err}", file.display()))
    } else {
        Ok(bytes)
    }
}

/// The header line of a session file (the first frame only, or the first line of an
/// uncompressed file), parsed; `None` when it has none.
pub fn header_of(file: &Path) -> Option<Value> {
    let line = if is_zstd(file) {
        let bytes = std::fs::read(file).ok()?;
        let first = decode_frames(&bytes, 1).ok()?;
        let end = first.iter().position(|b| *b == b'\n')?;
        first[..end].to_vec()
    } else {
        let handle = std::fs::File::open(file).ok()?;
        let mut line = Vec::new();
        std::io::BufReader::new(handle.take(MAX_HEADER))
            .read_until(b'\n', &mut line)
            .ok()?;
        if line.last() != Some(&b'\n') {
            return None;
        }
        line
    };
    serde_json::from_slice::<Value>(&line)
        .ok()
        .filter(Value::is_object)
}

/// The working directory a session file's header names, for a session that is read (a known
/// header, not a sub-agent's).
pub fn cwd_of(file: &Path) -> Option<String> {
    let header = header_of(file)?;
    check_header(&header).ok()?;
    if header["origin"] == "subagent" {
        return None;
    }
    header["cwd"]
        .as_str()
        .filter(|cwd| !cwd.is_empty())
        .map(str::to_string)
}

/// Whether the header is one this reader knows.
fn check_header(header: &Value) -> Result<(), Skip> {
    if header["type"] != "session" {
        return Err(Skip("its first line is not a session header".to_string()));
    }
    match header["version"].as_u64() {
        Some(VERSION) => Ok(()),
        Some(other) => Err(Skip(format!(
            "session format version {other}, this version reads {VERSION}"
        ))),
        None => Err(Skip("its header has no version".to_string())),
    }
}

/// Whether the recorded working directory `cwd` is `dir` or inside it; a Windows path is
/// compared with either separator and without case.
pub fn within(cwd: &str, dir: &Path) -> bool {
    let dir = dir.to_string_lossy();
    if is_windows_path(cwd) || is_windows_path(&dir) {
        let cwd = cwd.replace('\\', "/");
        let dir = dir.replace('\\', "/");
        let (cwd, dir) = (cwd.trim_end_matches('/'), dir.trim_end_matches('/'));
        let Some(head) = cwd.get(..dir.len()) else {
            return false;
        };
        return head.eq_ignore_ascii_case(dir)
            && (cwd.len() == dir.len() || cwd[dir.len()..].starts_with('/'));
    }
    Path::new(cwd).starts_with(dir.as_ref())
}

/// The text parts of a message's `content`.
fn text_of(content: &Value) -> String {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["type"] == "text")
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Appends `text` to `field` after a blank line, cut at [`CAP`].
fn append(field: &mut String, text: &str) {
    if text.is_empty() {
        return;
    }
    if !field.is_empty() {
        field.push_str("\n\n");
    }
    field.push_str(text);
    *field = clip(field, CAP);
}

/// The turns of one session as they are read.
#[derive(Default)]
struct Turns {
    /// The dsh turn the events are in (the last `turn/start`, until its `turn/end`).
    current: Option<i64>,
    /// Our turn of each dsh turn.
    of_dsh: HashMap<i64, usize>,
    /// Our last turn when it started outside a dsh turn (mapped by the next `turn/start`).
    pending: Option<usize>,
}

impl Turns {
    /// Our turn of the dsh turn `turn`, else the last one.
    fn target(&self, out: &Transcript, turn: Option<i64>) -> Option<usize> {
        turn.and_then(|t| self.of_dsh.get(&t).copied())
            .or_else(|| out.turns.len().checked_sub(1))
    }
}

/// Parses a session file's decoded lines; [`Skip`] for a header this reader does not know.
pub fn parse(bytes: &[u8]) -> Result<Transcript, Skip> {
    let complete = match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(end) => &bytes[..=end],
        None => &bytes[..0],
    };
    let mut out = Transcript {
        bytes: complete.len(),
        fingerprint: crate::fsutil::fingerprint(complete),
        partial_tail: complete.len() < bytes.len(),
        ..Transcript::default()
    };
    if out.partial_tail {
        out.skipped_lines += 1;
    }
    let mut lines = complete
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace));
    let Some(first) = lines.next() else {
        return Ok(out); // nothing complete yet
    };
    let header = serde_json::from_slice::<Value>(first).unwrap_or(Value::Null);
    check_header(&header)?;
    out.records = 1;
    out.session_id = header["id"].as_str().unwrap_or_default().to_string();
    out.cwd = header["cwd"]
        .as_str()
        .filter(|cwd| !cwd.is_empty())
        .map(str::to_string);
    out.started_at = header["createdAt"]
        .as_i64()
        .map(timestamp)
        .unwrap_or_default();
    if header["origin"] == "subagent" {
        return Ok(out);
    }
    let mut turns = Turns::default();
    for line in lines {
        let Ok(event @ Value::Object(_)) = serde_json::from_slice::<Value>(line) else {
            out.skipped_lines += 1;
            continue;
        };
        out.records += 1;
        let at = event["time"].as_i64().map(timestamp).unwrap_or_default();
        if !at.is_empty() {
            if out.started_at.is_empty() {
                out.started_at = at.clone();
            }
            out.ended_at = at.clone();
        }
        let data = &event["data"];
        match event["type"].as_str().unwrap_or_default() {
            "turn/start" => {
                turns.current = data["turn"].as_i64();
                if let (Some(turn), Some(ours)) = (turns.current, turns.pending.take()) {
                    turns.of_dsh.entry(turn).or_insert(ours);
                }
            }
            "turn/end" => turns.current = None,
            "user/message" if data["source"]["kind"] == "user" => {
                let text = text_of(&data["content"]);
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                if let Some(ours) = turns.current.and_then(|t| turns.of_dsh.get(&t).copied()) {
                    append(&mut out.turns[ours].user, text);
                    continue;
                }
                if out.title.is_empty() {
                    out.title = clip(text.lines().next().unwrap_or_default(), 120);
                }
                let n = out.turns.len();
                out.turns.push(Turn {
                    n,
                    at: at.clone(),
                    user: clip(text, CAP),
                    assistant: String::new(),
                    tools: Vec::new(),
                });
                match turns.current {
                    Some(turn) => {
                        turns.of_dsh.insert(turn, n);
                    }
                    None => turns.pending = Some(n),
                }
            }
            "assistant/message" => {
                let text = text_of(&data["message"]["content"]);
                if let Some(ours) = turns.target(&out, data["turn"].as_i64()) {
                    append(&mut out.turns[ours].assistant, text.trim());
                }
            }
            "tool/call" => {
                let Some(name) = data["name"].as_str().filter(|n| !n.is_empty()) else {
                    continue;
                };
                out.memory_writes
                    .extend(memory_writes(name, &data["arguments"], &at));
                if let Some(ours) = turns.target(&out, data["turn"].as_i64()) {
                    let tools = &mut out.turns[ours].tools;
                    if !tools.iter().any(|t| t == name) {
                        tools.push(name.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    if out.cwd.is_none() {
        out.turns.clear();
    }
    if out.session_id.is_empty() {
        out.session_id = "unknown".to_string();
    }
    Ok(out)
}

/// Reads a session file; a skipped one ([`Skip`]) is logged and read as empty.
pub fn read(file: &Path) -> Result<Transcript, String> {
    let bytes = load(file)?;
    Ok(parse(&bytes).unwrap_or_else(|Skip(why)| {
        crate::runtime::daemon::log(&format!("dsh session {} skipped: {why}", file.display()));
        Transcript::default()
    }))
}

/// The sessions of `root` whose working directory is `directory` or inside it, written at or
/// after `since` (seconds since the epoch): what dsh wrote there since then, ended when its
/// MCP connection closes (dsh has no session end event).
pub fn written_since(root: &Path, directory: &Path, since: u64) -> Vec<PathBuf> {
    units(root)
        .into_iter()
        .filter(|unit| unit.modified >= since)
        .filter(|unit| cwd_of(&unit.path).is_some_and(|cwd| within(&cwd, directory)))
        .map(|unit| unit.path)
        .collect()
}

/// dsh's sessions (`crate::memory::sessions::SessionSource`).
pub struct Sessions;

impl crate::memory::sessions::SessionSource for Sessions {
    fn units(&self, dir: &Path) -> Result<Vec<Unit>, String> {
        Ok(units(dir))
    }

    fn unit_id(&self, file: &Path) -> String {
        unit_id(file)
    }

    fn read(&self, file: &Path, _data_dir: &Path) -> Result<(Transcript, String), String> {
        let directory = file
            .parent()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        Ok((read(file)?, format!("dsh:{directory}/{name}")))
    }

    fn written_since(&self, root: &Path, dir: &Path, since: u64) -> Vec<PathBuf> {
        written_since(root, dir, since)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::sessions::SessionSource;
    use crate::runtime::mcp_tests::{TempDir, abs};
    use serde_json::json;

    const ID: &str = "session-0a1b2c3d-0000-4000-8000-000000000001";
    /// 2026-11-14T10:00:00Z in milliseconds.
    const T0: i64 = 1_794_650_400_000;

    fn header(cwd: Option<&str>) -> Value {
        let mut h = json!({"type": "session", "version": 4, "id": ID, "createdAt": T0,
            "isSeeded": false, "delegationDepth": 0});
        if let Some(cwd) = cwd {
            h["cwd"] = json!(cwd);
        }
        h
    }

    fn event(seq: i64, kind: &str, data: Value) -> Value {
        json!({"type": kind, "seq": seq, "time": T0 + seq * 1000, "data": data})
    }

    fn user(seq: i64, kind: &str, text: &str) -> Value {
        event(
            seq,
            "user/message",
            json!({"role": "user", "id": format!("m{seq}"),
                "content": [{"type": "text", "text": text}], "source": {"kind": kind}}),
        )
    }

    fn assistant(seq: i64, turn: i64, text: &str) -> Value {
        event(
            seq,
            "assistant/message",
            json!({"turn": turn, "step": 0, "message": {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "hidden reasoning"},
                {"type": "text", "text": text}],
                "source": {"kind": "model", "provider": "deepseek", "model": "deepseek-chat"}},
                "stream": false}),
        )
    }

    fn call(seq: i64, turn: i64, name: &str, arguments: Value) -> Value {
        event(
            seq,
            "tool/call",
            json!({"turn": turn, "step": 0, "callId": format!("c{seq}"), "name": name,
                "arguments": arguments.to_string()}),
        )
    }

    /// The events of a session: two turns, injected messages, a memory write, an unknown event.
    fn events() -> Vec<Value> {
        vec![
            event(1, "turn/start", json!({"turn": 1})),
            user(
                2,
                "agent-instructions",
                "You are dsh. Injected instructions.",
            ),
            user(3, "time-context", "It is Saturday."),
            user(4, "user", "Use tabs in this repository, never spaces."),
            call(
                5,
                1,
                "mcp__graphersal-memory__remember",
                json!({"slug": "tabs", "summary": "Tabs, never spaces"}),
            ),
            event(
                6,
                "tool/result",
                json!({"turn": 1, "step": 0, "message": {"toolCallId": "c5", "content": "ok"}}),
            ),
            assistant(7, 1, "Noted: tabs, never spaces."),
            event(8, "session/some-future-event", json!({"anything": true})),
            event(
                9,
                "turn/end",
                json!({"turn": 1, "reason": {"kind": "completed"}}),
            ),
            event(10, "turn/start", json!({"turn": 2})),
            user(11, "user", "Fix the parser."),
            call(12, 2, "read_file", json!({"path": "src/parser.rs"})),
            user(13, "user", "Also the lexer."),
            assistant(14, 2, "Fixed both."),
            user(15, "subagent-settled", "A sub-agent finished."),
            event(
                16,
                "turn/end",
                json!({"turn": 2, "reason": {"kind": "completed"}}),
            ),
        ]
    }

    fn lines(values: &[Value]) -> String {
        values.iter().map(|v| format!("{v}\n")).collect()
    }

    fn session_text(cwd: Option<&str>) -> String {
        let mut all = vec![header(cwd)];
        all.extend(events());
        lines(&all)
    }

    /// The text as dsh writes it compressed: the header line in one frame, then one frame per
    /// batch of events.
    fn zstd_frames(batches: &[String]) -> Vec<u8> {
        use ruzstd::encoding::{CompressionLevel, compress_to_vec};
        batches
            .iter()
            .flat_map(|batch| compress_to_vec(batch.as_bytes(), CompressionLevel::Fastest))
            .collect()
    }

    fn zstd_session(cwd: Option<&str>) -> Vec<u8> {
        let evs = events();
        zstd_frames(&[lines(&[header(cwd)]), lines(&evs[..8]), lines(&evs[8..])])
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_session_becomes_turns_with_tools_and_memories() {
        let cwd = abs("/w/app");
        let t = parse(session_text(Some(&cwd)).as_bytes()).unwrap();
        assert_eq!(t.session_id, ID);
        assert_eq!(t.cwd.as_deref(), Some(cwd.as_str()));
        assert_eq!(t.title, "Use tabs in this repository, never spaces.");
        assert_eq!(t.turns.len(), 2, "{:?}", t.turns);
        assert_eq!(
            t.turns[0].user,
            "Use tabs in this repository, never spaces."
        );
        assert_eq!(t.turns[0].at, timestamp(T0 + 4000));
        assert_eq!(t.turns[0].assistant, "Noted: tabs, never spaces.");
        assert_eq!(t.turns[0].tools, vec!["mcp__graphersal-memory__remember"]);
        // A second human message in dsh turn 2 joins our turn 1.
        assert_eq!(t.turns[1].n, 1);
        assert_eq!(t.turns[1].user, "Fix the parser.\n\nAlso the lexer.");
        assert_eq!(t.turns[1].assistant, "Fixed both.");
        assert_eq!(t.turns[1].tools, vec!["read_file"]);
        assert!(
            !t.turns
                .iter()
                .any(|turn| turn.user.contains("Injected") || turn.user.contains("sub-agent")),
            "injected messages are not the user's words"
        );
        assert!(!t.turns[0].assistant.contains("hidden reasoning"));
        assert_eq!(t.memory_writes.len(), 1);
        assert_eq!(t.memory_writes[0].slug, "tabs");
        assert_eq!(t.memory_writes[0].action, "remember");
        assert_eq!(t.memory_writes[0].at, timestamp(T0 + 5000));
        assert_eq!(t.started_at, "2026-11-14T10:00:00.000Z");
        assert_eq!(t.ended_at, timestamp(T0 + 16000));
        assert_eq!(t.records, 17, "the unknown event is read and skipped");
        assert_eq!(t.skipped_lines, 0);
        assert!(!t.partial_tail);
    }

    #[test]
    fn turn_numbers_come_from_turn_start() {
        // Answers name dsh turn 1 after turn 2 started: they belong to our turn 0.
        let text = lines(&[
            header(Some("/w")),
            event(1, "turn/start", json!({"turn": 1})),
            user(2, "user", "First."),
            event(
                3,
                "turn/end",
                json!({"turn": 1, "reason": {"kind": "completed"}}),
            ),
            event(4, "turn/start", json!({"turn": 2})),
            user(5, "user", "Second."),
            assistant(6, 1, "Late answer to the first."),
            call(7, 1, "grep", json!({})),
            // A dsh turn without a human message: its answer goes to the last turn.
            event(8, "turn/start", json!({"turn": 3})),
            user(9, "goal", "Keep going."),
            assistant(10, 3, "Continuing."),
            // A human message outside a turn is mapped by the next turn/start.
            event(
                11,
                "turn/end",
                json!({"turn": 3, "reason": {"kind": "completed"}}),
            ),
            user(12, "user", "Third."),
            event(13, "turn/start", json!({"turn": 4})),
            assistant(14, 4, "Third answer."),
        ]);
        let t = parse(text.as_bytes()).unwrap();
        let got: Vec<(&str, &str)> = t
            .turns
            .iter()
            .map(|turn| (turn.user.as_str(), turn.assistant.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("First.", "Late answer to the first."),
                ("Second.", "Continuing."),
                ("Third.", "Third answer."),
            ]
        );
        assert_eq!(t.turns[0].tools, vec!["grep"]);
    }

    #[test]
    fn zstd_frames_partial_lines_and_torn_frames() {
        let cwd = abs("/w/app");
        let plain = parse(session_text(Some(&cwd)).as_bytes()).unwrap();
        let bytes = zstd_session(Some(&cwd));
        let decoded = decode_frames(&bytes, usize::MAX).unwrap();
        assert_eq!(decoded, session_text(Some(&cwd)).as_bytes());
        assert_eq!(parse(&decoded).unwrap(), plain);
        // Only the first frame: the header line.
        assert_eq!(
            decode_frames(&bytes, 1).unwrap(),
            lines(&[header(Some(&cwd))]).as_bytes()
        );

        // A torn last frame: the frames before it are kept.
        let evs = events();
        let first_two = zstd_frames(&[lines(&[header(Some(&cwd))]), lines(&evs[..8])]);
        for cut in [1, 5, 12] {
            let torn = &bytes[..bytes.len() - cut];
            assert_eq!(
                decode_frames(torn, usize::MAX).unwrap(),
                decode_frames(&first_two, usize::MAX).unwrap(),
                "cut {cut}"
            );
        }
        let t = parse(&decode_frames(&bytes[..bytes.len() - 3], usize::MAX).unwrap()).unwrap();
        assert_eq!(t.turns.len(), 1);
        // A corrupt checksum ends the read too.
        let mut bad = bytes.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        assert_eq!(
            decode_frames(&bad, usize::MAX).unwrap(),
            decode_frames(&first_two, usize::MAX).unwrap()
        );
        // Garbage after the magic number, and a file that is not zstd at all.
        assert!(
            decode_frames(&[0x28, 0xB5, 0x2F, 0xFD, 1, 2, 3], usize::MAX)
                .unwrap()
                .is_empty()
        );
        assert!(decode_frames(b"{\"type\":\"session\"}\n", usize::MAX).is_err());
        assert!(decode_frames(b"", usize::MAX).unwrap().is_empty());

        // A partial last line is skipped and left out of the fingerprint.
        let text = session_text(Some(&cwd));
        let with_tail = format!("{text}{{\"type\": \"assistant/mes");
        let t = parse(with_tail.as_bytes()).unwrap();
        assert!(t.partial_tail);
        assert_eq!(t.skipped_lines, 1);
        assert_eq!(t.fingerprint, crate::fsutil::fingerprint(text.as_bytes()));
        assert_eq!(t.turns, plain.turns);
        // A bad line in the middle is skipped.
        let with_bad = text.replacen('\n', "\nnot json\n", 1);
        assert_eq!(parse(with_bad.as_bytes()).unwrap().skipped_lines, 1);
        // Only a partial header: nothing yet.
        assert!(parse(b"{\"type\":\"sess").unwrap().turns.is_empty());
    }

    #[test]
    fn unknown_headers_are_skipped_and_some_sessions_have_no_turns() {
        let mut v5 = header(Some("/w"));
        v5["version"] = json!(5);
        let text = lines(&[v5, user(1, "user", "Hello")]);
        assert!(matches!(parse(text.as_bytes()), Err(Skip(why)) if why.contains('5')));
        let text = lines(&[json!({"type": "turn/start", "data": {"turn": 1}})]);
        assert!(parse(text.as_bytes()).is_err());
        assert!(parse(b"not json\n").is_err());

        let mut sub = header(Some("/w"));
        sub["origin"] = json!("subagent");
        sub["delegationDepth"] = json!(1);
        let mut all = vec![sub];
        all.extend(events());
        let t = parse(lines(&all).as_bytes()).unwrap();
        assert!(t.turns.is_empty() && t.memory_writes.is_empty());

        let t = parse(session_text(None).as_bytes()).unwrap();
        assert!(t.turns.is_empty(), "no working directory, no project");
    }

    #[test]
    fn a_windows_working_directory() {
        let cwd = r"C:\Users\me\proj";
        let t = parse(session_text(Some(cwd)).as_bytes()).unwrap();
        assert_eq!(t.cwd.as_deref(), Some(cwd));
        assert_eq!(t.turns.len(), 2);
        assert!(within(cwd, Path::new(r"C:\Users\me\proj")));
        assert!(within(cwd, Path::new("c:/users/me")));
        assert!(within(cwd, Path::new(r"C:\Users\me\proj\")));
        assert!(!within(cwd, Path::new(r"C:\Users\me\pro")));
        assert!(!within(cwd, Path::new(r"C:\Users\other")));
        assert!(!within(r"C:\Users\me", Path::new(r"C:\Users\me\proj")));
    }

    #[test]
    fn units_and_their_files() {
        let tmp = TempDir::new("dsh-scan");
        let root = tmp.path().join("sessions");
        let cwd = tmp
            .path()
            .join("work")
            .join("app")
            .to_string_lossy()
            .to_string();
        let project = root.join("--app--");

        // The default: compressed.
        let zstd = project.join(ID).join(V4_ZSTD);
        write(&zstd, &zstd_session(Some(&cwd)));
        std::fs::write(project.join(ID).join("session.v3.jsonl.zstd"), b"old").unwrap();
        // Uncompressed (`sdk-minimal`).
        let plain_dir = project.join("session-0a1b2c3d-0000-4000-8000-000000000002");
        let plain = plain_dir.join(V4_PLAIN);
        write(&plain, session_text(Some(&cwd)).as_bytes());
        // Only older generations: no unit.
        let old = project.join("session-0a1b2c3d-0000-4000-8000-000000000003");
        write(
            &old.join("session.v3.jsonl"),
            session_text(Some(&cwd)).as_bytes(),
        );
        write(&old.join("session.jsonl"), b"{}\n");
        // Without a working directory.
        let no_cwd = root
            .join("_no-cwd")
            .join("session-0a1b2c3d-0000-4000-8000-000000000004")
            .join(V4_ZSTD);
        write(&no_cwd, &zstd_session(None));
        // A stray file at either level.
        std::fs::write(root.join("index.json"), b"{}").unwrap();
        std::fs::write(project.join("notes.txt"), b"x").unwrap();

        let mut want = vec![zstd.clone(), plain.clone(), no_cwd.clone()];
        want.sort();
        assert_eq!(scan(&root), want);
        assert_eq!(units(&root).len(), 3);
        assert!(scan(&tmp.path().join("missing")).is_empty());

        let source = Sessions;
        assert_eq!(
            source.unit_id(&zstd),
            "dsh-0a1b2c3d-0000-4000-8000-000000000001"
        );
        assert_eq!(unit_id(&project.join("odd").join(V4_PLAIN)), "dsh-odd");
        let (t, label) = source.read(&zstd, tmp.path()).unwrap();
        assert_eq!(t.turns.len(), 2);
        assert_eq!(label, format!("dsh:{ID}/{V4_ZSTD}"));
        assert_eq!(read(&plain).unwrap().turns, t.turns);
        assert!(read(&no_cwd).unwrap().turns.is_empty());

        // An unknown version: skipped, read as empty, not an error.
        let mut v5 = header(Some(&cwd));
        v5["version"] = json!(5);
        let future = project.join("session-future").join(V4_PLAIN);
        write(&future, lines(&[v5, user(1, "user", "Hi")]).as_bytes());
        assert_eq!(read(&future).unwrap(), Transcript::default());
        // A file that is not zstd: an error.
        let broken = project.join("session-broken").join(V4_ZSTD);
        write(&broken, b"plain text");
        assert!(read(&broken).is_err());

        assert_eq!(header_of(&zstd).unwrap()["id"], ID);
        assert_eq!(cwd_of(&plain).as_deref(), Some(cwd.as_str()));
        assert_eq!(cwd_of(&no_cwd), None);
        assert_eq!(cwd_of(&future), None);
    }

    #[test]
    fn both_v4_files_the_one_modified_last() {
        let tmp = TempDir::new("dsh-both");
        let dir = tmp.path().join(ID);
        let zstd = dir.join(V4_ZSTD);
        let plain = dir.join(V4_PLAIN);
        write(&zstd, &zstd_session(Some("/w")));
        write(&plain, session_text(Some("/w")).as_bytes());
        let set = |path: &Path, secs: u64| {
            let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(at)
                .unwrap();
        };
        set(&zstd, 1_000);
        set(&plain, 2_000);
        assert_eq!(session_file(&dir), Some(plain.clone()));
        set(&zstd, 3_000);
        assert_eq!(session_file(&dir), Some(zstd.clone()));
        set(&plain, 3_000);
        assert_eq!(session_file(&dir), Some(zstd.clone()), "a tie: compressed");
        assert_eq!(session_file(&tmp.path().join("missing")), None);
    }

    #[test]
    fn written_since_by_working_directory() {
        let tmp = TempDir::new("dsh-since");
        let root = tmp.path().join("sessions");
        let work = tmp.path().join("work");
        let app = work.join("app");
        let cwd = app.to_string_lossy().to_string();
        let path = root.join("--app--").join(ID).join(V4_ZSTD);
        write(&path, &zstd_session(Some(&cwd)));
        let windows = root
            .join("C-Users-me-proj")
            .join("session-win")
            .join(V4_PLAIN);
        write(&windows, session_text(Some(r"C:\Users\me\proj")).as_bytes());
        let no_cwd = root.join("_no-cwd").join("session-none").join(V4_ZSTD);
        write(&no_cwd, &zstd_session(None));
        let mut sub = header(Some(&cwd));
        sub["origin"] = json!("subagent");
        let sub_path = root.join("--app--").join("session-sub").join(V4_PLAIN);
        write(&sub_path, lines(&[sub]).as_bytes());

        assert_eq!(written_since(&root, &app, 0), vec![path.clone()]);
        assert_eq!(
            written_since(&root, &work, 0),
            vec![path.clone()],
            "a session in a directory of the project"
        );
        assert!(written_since(&root, &app, u64::MAX).is_empty());
        assert!(written_since(&root, &work.join("other"), 0).is_empty());
        assert_eq!(
            Sessions.written_since(&root, Path::new(r"c:\users\me\PROJ"), 0),
            vec![windows.clone()]
        );
    }
}
