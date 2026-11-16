//! Minimal-change edits of a YAML patch file (DeepSeek Harness's `cordis.patch.yml`): reading
//! goes through the `saphyr-parser` event parser, writing is a text edit that keeps every byte
//! of the user's file except the part changed. The file is never serialized again.
//!
//! The file's top level is a sequence of patch operations (mappings). dsh reads it with js-yaml
//! 4's JSON schema plus one scalar tag, `!!js` (`tag:yaml.org,2002:js`, a JavaScript expression);
//! duplicate mapping keys and unknown tags are errors there, and an empty or comments-only file
//! fails its boot (a missing file is no layer at all). [`parse`] reads the same way: plain
//! scalars resolve as the JSON schema does (`null`/`~`, `true`/`false` in their three spellings,
//! integers, floats), a `!!js` scalar becomes `{"__jsExpr": "<expression>"}` as in dsh, a
//! duplicate key, an unknown tag, a complex key or a second document is an error. A float that
//! JSON cannot hold (`.inf`, `.nan`) is kept as its text.
//!
//! The one edit is a **managed block** of whole top-level sequence items between
//! `# graphersal-memory:begin fingerprint=...` and `# graphersal-memory:end` comment lines, as in
//! `tomledit`: the fingerprint on the begin line is that of the block's text (line endings
//! normalized), so a hand edit inside is detected (and reported, never overwritten).
//! [`put_block`] appends it to a top-level block sequence, puts it in place of an empty flow
//! sequence `[]` (dsh's own template ends with one), or writes it into a text without YAML
//! content (empty, whitespace, comments). [`remove_block`] takes it out again and gives the
//! original bytes back; when no YAML content remains, it writes `[]` where the block was, so the
//! file stays valid for dsh. A UTF-8 byte order mark at the start is kept byte for byte; a
//! top-level list that does not start at column 0 is refused (the block would not join it).
//!
//! Every edit is checked by parsing the result: its items must be the ones before with exactly
//! our items added (or replaced, or removed), else the edit is refused (the file is left as it
//! is).

use std::collections::HashMap;

use saphyr_parser::{Event, Marker, Parser, ScalarStyle, Tag};
use serde_json::{Map, Value};

use crate::fsutil;

/// The first characters of the managed block's begin line.
pub const BEGIN: &str = "# graphersal-memory:begin";

/// The managed block's end line.
pub const END: &str = "# graphersal-memory:end";

/// The word on the begin line saying that `put_block` added a line break before the block (the
/// file did not end with one); `remove_block` takes it out again when the block ends the file.
const NEWLINE_ADDED: &str = "newline-added";

/// The UTF-8 byte order mark a file may start with (kept byte for byte; js-yaml skips it).
const BOM: &str = "\u{feff}";

/// Whether `at` is at the start of a line of `text` (a byte order mark before it counts as
/// nothing).
fn at_line_start(text: &str, at: usize) -> bool {
    at == 0 || text[..at].ends_with('\n') || &text[..at] == BOM
}

/// The key dsh gives a `!!js` scalar's expression.
pub const JS_EXPR: &str = "__jsExpr";

/// The shape of a patch file's top level that the managed block can go into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Top {
    /// No YAML content: empty, whitespace, comments, an empty document (invalid for dsh).
    Empty,
    /// An empty flow sequence `[]`.
    EmptyFlow,
    /// A block sequence (`- ...` items).
    Block,
}

/// The document of a YAML text, as JSON: `Null` for a text without a document (empty,
/// whitespace, comments only). `Err` names the problem (a syntax error with its line, a
/// duplicate key, an unknown tag, more than one document).
#[cfg(test)]
pub fn parse(text: &str) -> Result<Value, String> {
    Ok(scan(text)?.root.unwrap_or(Value::Null))
}

/// The shape of `text`'s top level, or `Err` when the managed block cannot go into it: not
/// valid YAML, a mapping or a scalar, a non-empty flow sequence.
pub fn top_level(text: &str) -> Result<Top, String> {
    shape(&scan(text)?)
}

/// The top-level items of `text` (each patch operation, as JSON; `!!js` scalars as
/// `{"__jsExpr": ...}`), empty for a text without content or `[]`. `Err` as [`top_level`].
pub fn items(text: &str) -> Result<Vec<Value>, String> {
    let scanned = scan(text)?;
    shape(&scanned)?;
    Ok(scanned.items())
}

/// A YAML double-quoted scalar of `text` (valid under every schema, also for paths with spaces,
/// backslashes or `:`): `"`, `\` and non-printable characters escaped. Booleans are written as
/// the bare words `true`/`false`.
pub fn string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' || ('\u{7f}'..='\u{9f}').contains(&c) || c == '\u{feff}' => {
                out.push_str(&format!("\\u{:04x}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The managed block for `body` (whole top-level sequence items in block style at column 0,
/// e.g. `- insert:\n    - id: ...\n`), marker lines included, ending with a newline.
#[cfg(test)]
pub fn wrap(body: &str) -> String {
    render(body, "\n", false)
}

/// The block with `eol` line endings, the begin line saying whether a line break was added
/// before it.
fn render(body: &str, eol: &str, newline_added: bool) -> String {
    let body = normalized(body);
    let mut body = body.replace('\n', eol);
    if !body.is_empty() && !body.ends_with(eol) {
        body.push_str(eol);
    }
    let flag = if newline_added {
        format!(" {NEWLINE_ADDED}")
    } else {
        String::new()
    };
    format!(
        "{BEGIN} fingerprint={}{flag} (managed by `graphersal-memory install`; \
         `graphersal-memory uninstall` removes it; an edit inside is reported as a conflict){eol}\
         {body}{END}{eol}",
        fsutil::fingerprint(normalized(&body).as_bytes())
    )
}

/// `text` with LF line endings.
fn normalized(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// The line ending `text` uses: CRLF when its first line ends with one, else LF.
fn line_ending(text: &str) -> &'static str {
    match text.find('\n') {
        Some(at) if text[..at].ends_with('\r') => "\r\n",
        _ => "\n",
    }
}

/// A managed block found in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Its first byte (the begin line).
    pub start: usize,
    /// One past its last byte (after the end line's line break, when it has one).
    pub end: usize,
    /// Whether its text still matches the fingerprint on its begin line.
    pub intact: bool,
    /// Its text between the marker lines, with LF line endings.
    pub body: String,
    /// One past the end marker (before what follows it on its line).
    marker_end: usize,
    /// Whether `put_block` added a line break before it.
    newline_added: bool,
    /// The line ending of its begin line (the one `put_block` wrote, also before the block when
    /// it added one).
    eol: &'static str,
}

/// The managed block of `text`: `Ok(None)` without one, `Err` for a begin line without an end
/// line.
pub fn find(text: &str) -> Result<Option<Found>, String> {
    let Some(start) = text
        .match_indices(BEGIN)
        .map(|(at, _)| at)
        .find(|at| at_line_start(text, *at))
    else {
        return Ok(None);
    };
    let line_end = text[start..]
        .find('\n')
        .map(|n| start + n + 1)
        .ok_or("the managed block has no end marker")?;
    let begin_line = &text[start..line_end];
    let eol = if begin_line.ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let words = begin_line
        .split('(')
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let mut recorded = String::new();
    let mut newline_added = false;
    for word in words {
        if let Some(value) = word.strip_prefix("fingerprint=") {
            recorded = value.to_string();
        } else if word == NEWLINE_ADDED {
            newline_added = true;
        }
    }
    let end_at = text[line_end..]
        .match_indices(END)
        .map(|(at, _)| line_end + at)
        .find(|at| text[..*at].ends_with('\n'))
        .ok_or("the managed block has no end marker")?;
    let body = normalized(&text[line_end..end_at]);
    let marker_end = end_at + END.len();
    let mut end = marker_end;
    if text[end..].starts_with("\r\n") {
        end += 2;
    } else if text[end..].starts_with('\n') {
        end += 1;
    }
    Ok(Some(Found {
        start,
        end,
        intact: fsutil::fingerprint(body.as_bytes()) == recorded,
        body,
        marker_end,
        newline_added,
        eol,
    }))
}

/// `text` with the managed block of `body` written: replaced in place where a block is; else
/// appended at the end of a top-level block sequence or of a text without YAML content (a line
/// break added first when the text does not end with one); else in place of an empty flow
/// sequence `[]` written at the start of a line (comments around it kept). The block uses the
/// file's line endings (CRLF when its first line ends with one). `Err`, with the file to be left
/// as it is, when the text is not valid YAML, its top level is a mapping, a scalar or a
/// non-empty flow sequence, `body` is not a block sequence, or the result would not be the
/// file's items with exactly `body`'s items added or replaced.
pub fn put_block(text: &str, body: &str) -> Result<String, String> {
    let wanted = scan(body)
        .ok()
        .filter(|scanned| shape(scanned) == Ok(Top::Block))
        .map(|scanned| scanned.items())
        .ok_or("the managed block's body must be a YAML block sequence")?;
    let before = scan(text)?;
    let top = shape(&before)?;
    let eol = line_ending(text);
    let (new, expected) = match find(text)? {
        Some(found) => {
            let block = render(body, eol, found.newline_added);
            let block = block.strip_suffix(eol).unwrap_or(&block);
            let new = format!(
                "{}{block}{}",
                &text[..found.start],
                &text[found.marker_end..]
            );
            (new, before.replaced(found.start, found.end, &wanted))
        }
        None => {
            if top == Top::Block {
                indented_root(text, &before)?;
            }
            let mut expected = before.items();
            expected.extend(wanted.iter().cloned());
            let new = match top {
                Top::Empty | Top::Block => {
                    if text.is_empty() || text.ends_with('\n') {
                        format!("{text}{}", render(body, eol, false))
                    } else {
                        format!("{text}{eol}{}", render(body, eol, true))
                    }
                }
                Top::EmptyFlow => {
                    let (open, close) = before.flow.ok_or("the file's `[]` was not found")?;
                    if &text[open..close] != "[]" || !at_line_start(text, open) {
                        return Err("the file's empty list is not written as `[]` at the start \
                                    of a line; write it so (or add the entries by hand)"
                            .to_string());
                    }
                    let block = render(body, eol, false);
                    let block = block.strip_suffix(eol).unwrap_or(&block);
                    format!("{}{block}{}", &text[..open], &text[close..])
                }
            };
            (new, expected)
        }
    };
    check(&new, &expected, true)?;
    Ok(new)
}

/// `text` without its managed block, or `None` when it has none. The original bytes come back:
/// a line break `put_block` added before the block is taken out when the block still ends the
/// file, and when no YAML content remains `[]` is written where the block was (followed by the
/// block's line break), so the file stays valid for dsh (and a replaced `[]` comes back as it
/// was). `Err` when the result would not be the file's items without exactly the block's items
/// (the check is skipped when the text itself is not valid YAML: then only the marked lines go).
pub fn remove_block(text: &str) -> Result<Option<String>, String> {
    let Some(found) = find(text)? else {
        return Ok(None);
    };
    let head = &text[..found.start];
    let tail = &text[found.end..];
    let rest = format!("{head}{tail}");
    // An empty document (`---` alone) is no content either.
    let has_content = scan(&rest)
        .map(|s| s.root.is_some_and(|root| !root.is_null()))
        .unwrap_or(true);
    let new = if !has_content {
        format!("{head}[]{}{tail}", &text[found.marker_end..found.end])
    } else if found.newline_added && tail.is_empty() {
        // Exactly the line break `put_block` added: a carriage return of the user's before it
        // stays.
        head.strip_suffix(found.eol).unwrap_or(head).to_string()
    } else {
        rest
    };
    if let Ok(before) = scan(text) {
        check(&new, &before.replaced(found.start, found.end, &[]), false)?;
    }
    Ok(Some(new))
}

/// `Err` when the top-level block sequence of `text` is indented: a block at column 0 would
/// not join it. Its first item's `-` must start its line (after a byte order mark).
fn indented_root(text: &str, scanned: &Scanned) -> Result<(), String> {
    let Some(&first) = scanned.item_starts.first() else {
        return Ok(());
    };
    let before = text[..first].trim_end();
    let Some(dash) = before.strip_suffix('-').map(str::len) else {
        return Ok(());
    };
    if at_line_start(text, dash) {
        return Ok(());
    }
    Err(
        "the file's top-level list is indented; the top-level list must start at column 0 \
         (each `- ` at the start of its line): move it there first"
            .to_string(),
    )
}

/// `Ok` when `new` parses with exactly the `expected` top-level items (and, `with_block`, our
/// block found intact in it).
fn check(new: &str, expected: &[Value], with_block: bool) -> Result<(), String> {
    let scanned = scan(new).map_err(|err| format!("the edit would not be valid YAML ({err})"))?;
    shape(&scanned)?;
    if scanned.items() != expected {
        return Err("the edit would change the file's other entries".to_string());
    }
    if with_block && !matches!(find(new), Ok(Some(found)) if found.intact) {
        return Err("the edit would not leave a whole managed block".to_string());
    }
    Ok(())
}

/// What a scan of a text found.
struct Scanned {
    /// The document, `None` without one.
    root: Option<Value>,
    /// Whether the root is a flow collection (`[...]`/`{...}`).
    root_flow: bool,
    /// The byte offset where each top-level sequence item starts.
    item_starts: Vec<usize>,
    /// The byte range of a top-level flow sequence, `[` to one past `]`.
    flow: Option<(usize, usize)>,
}

impl Scanned {
    /// The top-level sequence items (empty when the root is not a sequence).
    fn items(&self) -> Vec<Value> {
        match &self.root {
            Some(Value::Array(items)) => items.clone(),
            _ => Vec::new(),
        }
    }

    /// The items with those starting in `start..end` replaced by `with` (at the place of the
    /// first of them, or after the items before `start`).
    fn replaced(&self, start: usize, end: usize, with: &[Value]) -> Vec<Value> {
        let items = self.items();
        let mut out = Vec::with_capacity(items.len() + with.len());
        let mut placed = false;
        for (item, at) in items.into_iter().zip(self.item_starts.iter()) {
            if *at >= start && !placed {
                out.extend(with.iter().cloned());
                placed = true;
            }
            if *at < start || *at >= end {
                out.push(item);
            }
        }
        if !placed {
            out.extend(with.iter().cloned());
        }
        out
    }
}

/// The top level of a scan, or why the block cannot go there.
fn shape(scanned: &Scanned) -> Result<Top, String> {
    match &scanned.root {
        // An empty document (`---` alone) holds nothing either.
        None | Some(Value::Null) => Ok(Top::Empty),
        Some(Value::Array(items)) if scanned.root_flow => {
            if items.is_empty() {
                Ok(Top::EmptyFlow)
            } else {
                Err(
                    "the file's top level is a flow list (`[...]`) with entries; write it as \
                     a block list (`- ...` lines) first"
                        .to_string(),
                )
            }
        }
        Some(Value::Array(_)) => Ok(Top::Block),
        Some(Value::Object(_)) => {
            Err("the file's top level is a mapping, not a list of patch operations".to_string())
        }
        Some(_) => {
            Err("the file's top level is a single value, not a list of patch operations".into())
        }
    }
}

/// One open collection while scanning.
enum Frame {
    Seq(Vec<Value>, usize),
    Map(Map<String, Value>, Option<String>, usize),
}

/// Builds the JSON document from the parser's events.
#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    anchors: HashMap<usize, Value>,
    root: Option<Value>,
}

impl Builder {
    /// A finished node: stored under its anchor, then added to the open collection.
    fn complete(&mut self, value: Value, anchor: usize) -> Result<(), String> {
        if anchor > 0 {
            self.anchors.insert(anchor, value.clone());
        }
        match self.stack.last_mut() {
            None => self.root = Some(value),
            Some(Frame::Seq(items, _)) => items.push(value),
            Some(Frame::Map(map, pending, _)) => match pending.take() {
                Some(key) => {
                    map.insert(key, value);
                }
                None => {
                    let key = key_text(&value)?;
                    if map.contains_key(&key) {
                        return Err(format!("duplicated mapping key `{key}`"));
                    }
                    *pending = Some(key);
                }
            },
        }
        Ok(())
    }
}

/// A mapping key as text (as js-yaml turns keys into property names).
fn key_text(value: &Value) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Null => Ok("null".to_string()),
        Value::Bool(_) | Value::Number(_) => Ok(value.to_string()),
        _ => Err("a mapping key that is a list, a mapping or a `!!js` value".to_string()),
    }
}

/// Whether `tag` is the core schema's `!!<name>`.
fn is_core(tag: &Tag, name: &str) -> bool {
    tag.is_yaml_core_schema() && tag.suffix == name
}

/// The value of a scalar under dsh's schema.
fn scalar(text: &str, style: ScalarStyle, tag: Option<&Tag>) -> Result<Value, String> {
    let Some(tag) = tag else {
        return Ok(if style == ScalarStyle::Plain {
            plain(text)
        } else {
            Value::String(text.to_string())
        });
    };
    if tag.handle.is_empty() && tag.suffix == "!" || is_core(tag, "str") {
        return Ok(Value::String(text.to_string()));
    }
    if is_core(tag, "js") {
        let mut map = Map::new();
        map.insert(JS_EXPR.to_string(), Value::String(text.to_string()));
        return Ok(Value::Object(map));
    }
    let resolved = plain(text);
    let fits = match tag.suffix.as_str() {
        "null" => resolved.is_null(),
        "bool" => resolved.is_boolean(),
        "int" => resolved.is_i64() || resolved.is_u64(),
        "float" => resolved.is_number() || is_special_float(text),
        _ => false,
    };
    if tag.is_yaml_core_schema() && fits {
        return Ok(resolved);
    }
    Err(format!("the tag `{tag}` on `{text}` is not one dsh knows"))
}

/// Whether `text` is `.inf`, `-.inf`, `.nan` and their spellings.
fn is_special_float(text: &str) -> bool {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    matches!(unsigned, ".inf" | ".Inf" | ".INF") || matches!(text, ".nan" | ".NaN" | ".NAN")
}

/// A plain scalar resolved as js-yaml's JSON schema does.
fn plain(text: &str) -> Value {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        _ => {}
    }
    if let Some(number) = integer(text) {
        return number;
    }
    if is_special_float(text) {
        return Value::String(text.to_string());
    }
    let first_ok = text
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'));
    let chars_ok = text
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E' | '_'));
    if first_ok && chars_ok && text.chars().any(|c| c.is_ascii_digit()) {
        let clean: String = text.chars().filter(|c| *c != '_').collect();
        if let Ok(float) = clean.parse::<f64>() {
            return serde_json::Number::from_f64(float)
                .map(Value::Number)
                .unwrap_or_else(|| Value::String(text.to_string()));
        }
    }
    Value::String(text.to_string())
}

/// A plain integer: decimal, `0b`, `0o` or `0x`, with an optional sign and `_` between digits.
fn integer(text: &str) -> Option<Value> {
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (radix, digits) = if let Some(rest) = unsigned.strip_prefix("0b") {
        (2, rest)
    } else if let Some(rest) = unsigned.strip_prefix("0o") {
        (8, rest)
    } else if let Some(rest) = unsigned.strip_prefix("0x") {
        (16, rest)
    } else {
        (10, unsigned)
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') {
        return None;
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    if !clean.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    let magnitude = i128::from_str_radix(&clean, radix).ok()?;
    let value = if negative { -magnitude } else { magnitude };
    match i64::try_from(value) {
        Ok(small) => Some(Value::from(small)),
        Err(_) => u64::try_from(value)
            .ok()
            .map(Value::from)
            .or_else(|| serde_json::Number::from_f64(value as f64).map(Value::Number)),
    }
}

/// Scan `text`: its document as JSON, where its top-level items start, its flow sequence.
fn scan(text: &str) -> Result<Scanned, String> {
    let offsets: Vec<usize> = text
        .char_indices()
        .map(|(at, _)| at)
        .chain(std::iter::once(text.len()))
        .collect();
    let byte = |marker: Marker| offsets.get(marker.index()).copied().unwrap_or(text.len());
    let mut builder = Builder::default();
    let mut documents = 0;
    let mut root_flow = false;
    let mut root_is_seq = false;
    let mut item_starts = Vec::new();
    let mut flow_open = None;
    let mut flow = None;
    for event in Parser::new_from_str(text) {
        let (event, span) = event.map_err(|err| err.to_string())?;
        let at = byte(span.start);
        let starts_node = matches!(
            event,
            Event::Alias(_)
                | Event::Scalar(..)
                | Event::SequenceStart(..)
                | Event::MappingStart(..)
        );
        if starts_node && root_is_seq && builder.stack.len() == 1 {
            item_starts.push(at);
        }
        match event {
            Event::Nothing | Event::StreamStart | Event::StreamEnd | Event::DocumentEnd => {}
            Event::DocumentStart(_) => {
                documents += 1;
                if documents > 1 {
                    return Err("the file holds more than one YAML document".to_string());
                }
            }
            Event::Alias(id) => {
                let value = builder
                    .anchors
                    .get(&id)
                    .cloned()
                    .ok_or("an alias to an unknown anchor")?;
                builder.complete(value, 0)?;
            }
            Event::Scalar(value, style, anchor, tag) => {
                let value = scalar(&value, style, tag.as_deref())?;
                builder.complete(value, anchor)?;
            }
            Event::SequenceStart(anchor, tag) => {
                if let Some(tag) = tag.as_deref()
                    && !is_core(tag, "seq")
                {
                    return Err(format!("the tag `{tag}` on a list is not one dsh knows"));
                }
                if builder.stack.is_empty() {
                    root_is_seq = true;
                    root_flow = text[at..].starts_with('[');
                    flow_open = root_flow.then_some(at);
                }
                builder.stack.push(Frame::Seq(Vec::new(), anchor));
            }
            Event::MappingStart(anchor, tag) => {
                if let Some(tag) = tag.as_deref()
                    && !is_core(tag, "map")
                {
                    return Err(format!("the tag `{tag}` on a mapping is not one dsh knows"));
                }
                if builder.stack.is_empty() {
                    root_flow = text[at..].starts_with('{');
                }
                builder.stack.push(Frame::Map(Map::new(), None, anchor));
            }
            Event::SequenceEnd | Event::MappingEnd => {
                let (value, anchor) = match builder.stack.pop() {
                    Some(Frame::Seq(items, anchor)) => (Value::Array(items), anchor),
                    Some(Frame::Map(map, None, anchor)) => (Value::Object(map), anchor),
                    Some(Frame::Map(_, Some(key), _)) => {
                        return Err(format!("the key `{key}` has no value"));
                    }
                    None => return Err("a collection ends that never started".to_string()),
                };
                if builder.stack.is_empty()
                    && let Some(open) = flow_open
                {
                    // The span runs on over a comment after the `]`; the `]` is its start.
                    let close = if text[at..].starts_with(']') {
                        at + 1
                    } else {
                        at
                    };
                    flow = Some((open, close.max(open)));
                }
                builder.complete(value, anchor)?;
            }
        }
    }
    Ok(Scanned {
        root: builder.root,
        root_flow,
        item_starts,
        flow,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const USER: &str = "# user overrides
- id: agent-default-model
  config:
    provider: deepseek-official
    model: deepseek-flash
- insert:
    - id: demo-mcp
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: demo
        transport: stdio
        command: node
        args: ['server.js']
- id: hmr
  disabled: !!js \"process.env.CI === 'true'\"
";

    const TEMPLATE: &str = "# DeepSeek Harness patch file.
# Each item is a patch operation.
[]
";

    fn body() -> String {
        format!(
            "- insert:\n    - id: graphersal-memory\n      name: '@deepseek-ai/dsh-mcp-client'\n      \
             config:\n        serverName: graphersal-memory\n        transport: stdio\n        \
             command: {}\n        args: ['mcp']\n",
            string("/Users/me/my tools/graphersal-memory")
        )
    }

    fn ours() -> Value {
        json!({"insert": [{
            "id": "graphersal-memory",
            "name": "@deepseek-ai/dsh-mcp-client",
            "config": {
                "serverName": "graphersal-memory",
                "transport": "stdio",
                "command": "/Users/me/my tools/graphersal-memory",
                "args": ["mcp"],
            },
        }]})
    }

    /// Install then uninstall gives `text` back, byte for byte; returns the installed text.
    fn round_trip(text: &str) -> String {
        let new = put_block(text, &body()).unwrap();
        let mut expected = items(text).unwrap();
        expected.push(ours());
        assert_eq!(items(&new).unwrap(), expected, "{new}");
        let found = find(&new).unwrap().unwrap();
        assert!(found.intact);
        assert_eq!(found.body, body());
        assert_eq!(put_block(&new, &body()).unwrap(), new, "idempotent");
        assert_eq!(
            remove_block(&new).unwrap().as_deref(),
            Some(text),
            "{new:?}"
        );
        new
    }

    #[test]
    fn parses_dsh_content_with_js_scalars() {
        let items = items(USER).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["config"]["model"], "deepseek-flash");
        assert_eq!(items[1]["insert"][0]["config"]["serverName"], "demo");
        assert_eq!(
            items[2]["disabled"],
            json!({"__jsExpr": "process.env.CI === 'true'"})
        );
        assert_eq!(top_level(USER), Ok(Top::Block));
    }

    #[test]
    fn json_schema_scalars() {
        let doc = parse(
            "- {a: null, b: ~, c: True, d: FALSE, e: 12, f: -0x1F, g: 1.5e3, h: yes, i: '12', \
             j: !!str 3, k: 0o17, l: 1_000, m: .inf, n: , o: '', p: 1.2.3}\n",
        )
        .unwrap();
        assert_eq!(
            doc,
            json!([{"a": null, "b": null, "c": true, "d": false, "e": 12, "f": -31,
                    "g": 1500.0, "h": "yes", "i": "12", "j": "3", "k": 15, "l": 1000,
                    "m": ".inf", "n": null, "o": "", "p": "1.2.3"}])
        );
    }

    #[test]
    fn errors_like_dsh() {
        assert!(
            parse("- id: a\n  id: b\n")
                .unwrap_err()
                .contains("duplicated")
        );
        assert!(parse("- !custom x\n").unwrap_err().contains("tag"));
        assert!(parse("- !!int abc\n").unwrap_err().contains("tag"));
        assert!(
            parse("- a\n---\n- b\n")
                .unwrap_err()
                .contains("more than one")
        );
        assert!(parse("- [a\n").is_err());
        assert!(parse("- ? [a]\n  : b\n").is_err());
        assert_eq!(parse("# only a comment\n").unwrap(), Value::Null);
        assert_eq!(parse("").unwrap(), Value::Null);
        assert_eq!(
            parse("- &x {a: 1}\n- *x\n").unwrap(),
            json!([{"a": 1}, {"a": 1}])
        );
    }

    #[test]
    fn appends_to_a_block_sequence_and_removes_byte_identical() {
        let new = round_trip(USER);
        assert!(new.starts_with(USER));
        assert!(!new.contains(NEWLINE_ADDED));
        // Without the final newline: one is added and taken out again.
        let bare = USER.trim_end_matches('\n');
        let new = round_trip(bare);
        assert!(new.contains(NEWLINE_ADDED));
        // Trailing comments after the last item stay above the block.
        let trailing = format!("{USER}# keep this\n\n# and this\n");
        let new = round_trip(&trailing);
        assert!(new.starts_with(&trailing));
        round_trip(trailing.trim_end());
    }

    #[test]
    fn keeps_crlf() {
        let crlf = USER.replace('\n', "\r\n");
        let new = round_trip(&crlf);
        assert!(!new.replace("\r\n", "").contains('\n'), "{new:?}");
        round_trip(crlf.trim_end());
        let template = TEMPLATE.replace('\n', "\r\n");
        let new = round_trip(&template);
        assert!(!new.replace("\r\n", "").contains('\n'), "{new:?}");
        // The fingerprint does not depend on the line endings.
        let lf = new.replace("\r\n", "\n");
        assert!(find(&lf).unwrap().unwrap().intact);
    }

    #[test]
    fn replaces_the_empty_template() {
        let new = round_trip(TEMPLATE);
        assert!(new.starts_with("# DeepSeek Harness patch file.\n# Each item"));
        assert!(!new.contains("[]"));
        assert!(new.ends_with(&format!("{END}\n")));
        round_trip("[]");
        round_trip("[]\n");
        round_trip("# head\n[] # nothing yet\n# tail\n");
        round_trip("---\n[]\n");
        let new = round_trip("[] # nothing yet");
        assert!(new.ends_with(&format!("{END} # nothing yet")));
    }

    #[test]
    fn empty_and_comment_only_texts_get_the_block_and_then_an_empty_list() {
        for (text, back) in [
            ("", "[]\n"),
            ("\n", "\n[]\n"),
            ("# my patches\n", "# my patches\n[]\n"),
            ("# my patches", "# my patches\n[]\n"),
        ] {
            let new = put_block(text, &body()).unwrap();
            assert!(new.starts_with(text));
            assert_eq!(items(&new).unwrap(), vec![ours()]);
            assert_eq!(remove_block(&new).unwrap().as_deref(), Some(back));
            assert_eq!(top_level(back), Ok(Top::EmptyFlow));
        }
        let crlf = put_block("# a\r\n", &body()).unwrap();
        assert_eq!(
            remove_block(&crlf).unwrap().as_deref(),
            Some("# a\r\n[]\r\n")
        );
    }

    #[test]
    fn explicit_document_start_with_a_block_sequence() {
        round_trip("%YAML 1.2\n---\n- id: hmr\n  enabled: false\n");
        let new = put_block("---\n# nothing yet\n", &body()).unwrap();
        assert_eq!(items(&new).unwrap(), vec![ours()]);
        assert_eq!(
            remove_block(&new).unwrap().as_deref(),
            Some("---\n# nothing yet\n[]\n")
        );
    }

    #[test]
    fn replaces_an_existing_block_in_place() {
        let installed = put_block(USER, "- id: graphersal-old\n  disabled: true\n").unwrap();
        let edited = format!("{installed}- id: later\n  disabled: false\n");
        let new = put_block(&edited, &body()).unwrap();
        let items = items(&new).unwrap();
        assert_eq!(items.len(), 5);
        assert_eq!(items[3], ours());
        assert_eq!(items[4]["id"], "later");
        assert!(new.ends_with("- id: later\n  disabled: false\n"));
        // Removed from the middle: the user's later item stays.
        let removed = remove_block(&new).unwrap().unwrap();
        assert_eq!(removed, format!("{USER}- id: later\n  disabled: false\n"));
        // The added newline is kept when an item follows the block.
        let bare = put_block(USER.trim_end(), &body()).unwrap();
        let later = format!("{bare}- id: later\n");
        let removed = remove_block(&later).unwrap().unwrap();
        assert_eq!(removed, format!("{}\n- id: later\n", USER.trim_end()));
    }

    #[test]
    fn detects_a_hand_edit_inside_the_block() {
        let new = put_block(USER, &body()).unwrap();
        let edited = new.replace("args: ['mcp']", "args: ['mcp', '--verbose']");
        let found = find(&edited).unwrap().unwrap();
        assert!(!found.intact);
        assert_eq!(remove_block(&edited).unwrap().as_deref(), Some(USER));
    }

    #[test]
    fn begin_without_end_is_an_error() {
        let new = put_block(USER, &body()).unwrap();
        let cut = new.replace(&format!("{END}\n"), "");
        assert!(find(&cut).is_err());
        assert!(put_block(&cut, &body()).is_err());
        assert!(remove_block(&cut).is_err());
        assert_eq!(remove_block(USER), Ok(None));
    }

    #[test]
    fn refuses_what_it_cannot_edit_safely() {
        for text in [
            "id: hmr\ndisabled: true\n",
            "just a string\n",
            "[{id: hmr}]\n",
            "- id: a\n  id: b\n",
            "- [unclosed\n",
            "- a\n---\n- b\n",
            "  []\n",
            "[ ]\n",
            "--- []\n",
            "- a\n...\n# after the end\n",
        ] {
            assert!(put_block(text, &body()).is_err(), "{text:?}");
        }
        assert!(top_level("id: hmr\n").is_err());
        assert!(items("[{id: hmr}]").is_err());
        // An indented sequence: a block at column 0 would not join it.
        assert!(put_block("  - id: a\n  - id: b\n", &body()).is_err());
        // The body must be a block sequence.
        assert!(put_block(USER, "id: x\n").is_err());
        assert!(put_block(USER, "[{id: x}]\n").is_err());
    }

    #[test]
    fn a_lone_carriage_return_of_the_user_stays() {
        // Lone CR line breaks, no final LF: install adds an LF before the block, uninstall
        // takes exactly that LF out again, also after an edit elsewhere in the file.
        let text = "- id: x\r- id: y\r";
        let new = put_block(text, &body()).unwrap();
        assert!(
            new.starts_with(text) && new.contains(NEWLINE_ADDED),
            "{new:?}"
        );
        assert_eq!(remove_block(&new).unwrap().as_deref(), Some(text));
        let edited = new.replacen("- id: x\r", "- id: z\r", 1);
        assert_eq!(
            remove_block(&edited).unwrap().as_deref(),
            Some("- id: z\r- id: y\r")
        );
        // A CRLF file without its final line break gets CRLF added and taken out.
        let crlf = "- id: x\r\n- id: y";
        let new = put_block(crlf, &body()).unwrap();
        assert!(new.starts_with("- id: x\r\n- id: y\r\n# graphersal-memory:begin"));
        assert_eq!(remove_block(&new).unwrap().as_deref(), Some(crlf));
    }

    #[test]
    fn a_byte_order_mark_is_kept() {
        for text in [
            "\u{feff}- id: hmr\n  disabled: true\n",
            "\u{feff}[]\n",
            "\u{feff}[]",
            "\u{feff}# head\n[]\n",
        ] {
            let new = round_trip(text);
            assert!(new.starts_with(BOM), "{new:?}");
            assert_eq!(new.matches(BOM).count(), 1, "{new:?}");
        }
        for (text, back) in [
            ("\u{feff}", "\u{feff}\n[]\n"),
            ("\u{feff}\n", "\u{feff}\n[]\n"),
        ] {
            let new = put_block(text, &body()).unwrap();
            assert_eq!(items(&new).unwrap(), vec![ours()]);
            assert_eq!(remove_block(&new).unwrap().as_deref(), Some(back));
        }
    }

    #[test]
    fn an_indented_top_level_list_is_refused_with_its_reason() {
        for text in [
            "  - id: a\n  - id: b\n",
            "\u{feff}  - id: a\n",
            "# head\n    - id: a\n",
        ] {
            let err = put_block(text, &body()).unwrap_err();
            assert!(err.contains("must start at column 0"), "{text:?}: {err}");
        }
        // An item whose content starts on the next line is at column 0 all the same.
        round_trip("-\n  id: a\n");
    }

    #[test]
    fn quotes_scalars_for_yaml() {
        for text in [
            "C:\\Program Files\\graphersal-memory.exe",
            "/home/me/a: b/#c",
            "it's \"quoted\"",
            "tab\tand\u{7f}del",
            "true",
        ] {
            let doc = parse(&format!("- {}\n", string(text))).unwrap();
            assert_eq!(doc, json!([text]));
        }
    }
}
