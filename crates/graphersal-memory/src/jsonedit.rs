//! Minimal-change edits of JSON text: insert or remove one member of an object, append or remove
//! one element of an array, and leave every other byte of the file as it was.
//!
//! `install` writes into the harness's own configuration files (`~/.claude.json`,
//! `~/.claude/settings.json`). Re-serializing them would reorder or reformat the user's text
//! (`serde_json` here has no `preserve_order`), so this module locates the spans of the members
//! and elements in the text and edits only there. An insertion followed by the removal of the
//! same member gives the original bytes back. The result of every edit is parsed again before
//! it is returned, so an edit never produces invalid JSON.
//!
//! JSONC (opencode's `opencode.jsonc`, and comments people put into `opencode.json`): `//` and
//! `/* */` comments and trailing commas are accepted. They are blanked out of a copy of the
//! text with the same byte offsets ([`clean`]); the spans are located in that copy and the
//! edits applied to the original, so every comment stays where it was. Strict JSON in gives
//! strict JSON out: an edit never adds a comment or a trailing comma.

use serde_json::Value;

/// A JSON value located in the text: its byte span and, for containers, its children.
#[derive(Debug, Clone)]
pub struct Node {
    /// The first byte of the value.
    pub start: usize,
    /// One past the last byte of the value.
    pub end: usize,
    /// What the value is.
    pub kind: Kind,
}

/// The kind of a located value.
#[derive(Debug, Clone)]
pub enum Kind {
    /// An object and its members in text order.
    Object(Vec<Member>),
    /// An array and its elements in text order.
    Array(Vec<Node>),
    /// A string, number, `true`, `false` or `null`.
    Scalar,
}

/// One member of an object.
#[derive(Debug, Clone)]
pub struct Member {
    /// The decoded key.
    pub key: String,
    /// The first byte of the key's opening quote.
    pub key_start: usize,
    /// The member's value.
    pub value: Node,
}

impl Node {
    /// The members, when this is an object.
    pub fn members(&self) -> Option<&[Member]> {
        match &self.kind {
            Kind::Object(members) => Some(members),
            _ => None,
        }
    }

    /// The elements, when this is an array.
    pub fn elements(&self) -> Option<&[Node]> {
        match &self.kind {
            Kind::Array(elements) => Some(elements),
            _ => None,
        }
    }

    /// The value of the member `key`, when this is an object that has it.
    pub fn get(&self, key: &str) -> Option<&Node> {
        self.members()?
            .iter()
            .find(|member| member.key == key)
            .map(|member| &member.value)
    }
}

/// `text` with its comments and trailing commas replaced by spaces (line ends kept), so every
/// byte offset stays the same; strict JSON comes back unchanged.
pub fn clean(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = bytes.to_vec();
    let mut i = 0;
    // The position of the last comma outside strings, while only blanks followed it.
    let mut pending_comma: Option<usize> = None;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                pending_comma = None;
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    if bytes[i] != b'\r' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let start = i;
                i += 2;
                while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                let end = (i + 2).min(bytes.len());
                for byte in &mut out[start..end] {
                    if *byte != b'\n' && *byte != b'\r' {
                        *byte = b' ';
                    }
                }
                i = end;
            }
            b',' => {
                pending_comma = Some(i);
                i += 1;
            }
            b'}' | b']' => {
                if let Some(comma) = pending_comma.take() {
                    out[comma] = b' ';
                }
                i += 1;
            }
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            _ => {
                pending_comma = None;
                i += 1;
            }
        }
    }
    // Only ASCII bytes outside strings were replaced by ASCII spaces: still UTF-8.
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// The value of JSON or JSONC `text` (comments and trailing commas allowed).
pub fn value_of(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(&clean(text))
}

/// Parses `text` (JSON or JSONC) into located nodes. The whole text must be one JSON value
/// (surrounding whitespace and comments allowed).
pub fn parse(text: &str) -> Result<Node, String> {
    let cleaned = clean(text);
    parse_clean(&cleaned)
}

/// [`parse`] of a text [`clean`] already went over.
fn parse_clean(text: &str) -> Result<Node, String> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        text,
        pos: 0,
        depth: 0,
    };
    parser.skip_ws();
    let node = parser.value()?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(format!("unexpected text at byte {}", parser.pos));
    }
    Ok(node)
}

/// The deepest nesting accepted (configuration files are shallow).
const MAX_DEPTH: usize = 128;

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    pos: usize,
    depth: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while let Some(b) = self.bytes.get(self.pos) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at byte {}", byte as char, self.pos))
        }
    }

    fn value(&mut self) -> Result<Node, String> {
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => {
                let start = self.pos;
                self.string()?;
                Ok(Node {
                    start,
                    end: self.pos,
                    kind: Kind::Scalar,
                })
            }
            Some(_) => self.literal(),
            None => Err("unexpected end of the text".to_string()),
        }
    }

    fn enter(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(format!("nested deeper than {MAX_DEPTH} levels"));
        }
        Ok(())
    }

    fn object(&mut self) -> Result<Node, String> {
        self.enter()?;
        let start = self.pos;
        self.expect(b'{')?;
        let mut members = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
        } else {
            loop {
                self.skip_ws();
                let key_start = self.pos;
                let raw = self.string()?;
                let key: String = serde_json::from_str(raw)
                    .map_err(|err| format!("bad key at byte {key_start}: {err}"))?;
                self.skip_ws();
                self.expect(b':')?;
                self.skip_ws();
                let value = self.value()?;
                members.push(Member {
                    key,
                    key_start,
                    value,
                });
                self.skip_ws();
                match self.peek() {
                    Some(b',') => self.pos += 1,
                    Some(b'}') => {
                        self.pos += 1;
                        break;
                    }
                    _ => return Err(format!("expected ',' or '}}' at byte {}", self.pos)),
                }
            }
        }
        self.depth -= 1;
        Ok(Node {
            start,
            end: self.pos,
            kind: Kind::Object(members),
        })
    }

    fn array(&mut self) -> Result<Node, String> {
        self.enter()?;
        let start = self.pos;
        self.expect(b'[')?;
        let mut elements = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
        } else {
            loop {
                self.skip_ws();
                elements.push(self.value()?);
                self.skip_ws();
                match self.peek() {
                    Some(b',') => self.pos += 1,
                    Some(b']') => {
                        self.pos += 1;
                        break;
                    }
                    _ => return Err(format!("expected ',' or ']' at byte {}", self.pos)),
                }
            }
        }
        self.depth -= 1;
        Ok(Node {
            start,
            end: self.pos,
            kind: Kind::Array(elements),
        })
    }

    /// A string; returns its raw text including the quotes.
    fn string(&mut self) -> Result<&str, String> {
        let start = self.pos;
        self.expect(b'"')?;
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\\') => self.pos += 2,
                Some(_) => self.pos += 1,
                None => return Err(format!("unterminated string at byte {start}")),
            }
        }
        self.text
            .get(start..self.pos)
            .ok_or_else(|| format!("bad string at byte {start}"))
    }

    fn literal(&mut self) -> Result<Node, String> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'+' | b'.') {
                self.pos += 1;
            } else {
                break;
            }
        }
        let raw = self.text.get(start..self.pos).unwrap_or_default();
        if raw.is_empty() || serde_json::from_str::<Value>(raw).is_err() {
            return Err(format!("unexpected value at byte {start}"));
        }
        Ok(Node {
            start,
            end: self.pos,
            kind: Kind::Scalar,
        })
    }
}

/// The text's indentation style, guessed from its first indented line: `Some(unit)` for
/// multi-line text, `None` for text on one line.
pub fn indent_unit(text: &str) -> Option<String> {
    if !text.trim_end().contains('\n') {
        return None;
    }
    for line in text.lines().skip(1) {
        let indent: String = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        if !indent.is_empty() && indent.len() < line.len() {
            return Some(indent);
        }
    }
    Some("  ".to_string())
}

/// `value` as JSON text at nesting `depth` in the style of `unit` (pretty with that unit, or
/// compact on one line).
pub fn render(value: &Value, unit: Option<&str>, depth: usize) -> String {
    let Some(unit) = unit else {
        return compact(value);
    };
    // The layout of `JSON.stringify(value, null, unit)` and serde_json's pretty printer.
    let inner = unit.repeat(depth + 1);
    let outer = unit.repeat(depth);
    match value {
        Value::Object(map) if !map.is_empty() => {
            let members: Vec<String> = map
                .iter()
                .map(|(key, value)| {
                    format!(
                        "{inner}{}: {}",
                        Value::from(key.as_str()),
                        render(value, Some(unit), depth + 1)
                    )
                })
                .collect();
            format!("{{\n{}\n{outer}}}", members.join(",\n"))
        }
        Value::Array(items) if !items.is_empty() => {
            let items: Vec<String> = items
                .iter()
                .map(|item| format!("{inner}{}", render(item, Some(unit), depth + 1)))
                .collect();
            format!("[\n{}\n{outer}]", items.join(",\n"))
        }
        other => compact(other),
    }
}

/// Compact JSON with a space after `:` and `,` (the one-line style people write).
fn compact(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let members: Vec<String> = map
                .iter()
                .map(|(key, value)| format!("{}: {}", Value::from(key.as_str()), compact(value)))
                .collect();
            format!("{{{}}}", members.join(", "))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(compact).collect();
            format!("[{}]", items.join(", "))
        }
        other => other.to_string(),
    }
}

/// The node at `path` (a chain of object keys from the root).
pub fn locate<'a>(root: &'a Node, path: &[&str]) -> Option<&'a Node> {
    let mut node = root;
    for key in path {
        node = node.get(key)?;
    }
    Some(node)
}

/// Whether `text` is only JSON whitespace.
fn blank(text: &str) -> bool {
    text.bytes()
        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
}

/// Checks that `text` is valid JSON or JSONC (the guard after every edit).
fn checked(text: String) -> Result<String, String> {
    value_of(&text).map_err(|err| format!("the edit would produce invalid JSON: {err}"))?;
    Ok(text)
}

/// Splices `replacement` into `text` at `start..end`.
fn splice(text: &str, start: usize, end: usize, replacement: &str) -> Result<String, String> {
    let head = text.get(..start).ok_or("bad edit position")?;
    let tail = text.get(end..).ok_or("bad edit position")?;
    Ok(format!("{head}{replacement}{tail}"))
}

/// The nesting depth of the node at `path` (the root is 0).
fn depth_of(path: &[&str]) -> usize {
    path.len()
}

/// Sets the member `key` of the object at `path` to `value`: replaces its value when it exists,
/// else appends the member. Objects missing along `path` are created. Returns the new text and
/// the keys of the objects it created (as dotted paths), or `None` when nothing changes.
pub fn set_member(
    text: &str,
    path: &[&str],
    key: &str,
    value: &Value,
) -> Result<Option<(String, Vec<String>)>, String> {
    let cleaned = clean(text);
    let root = parse_clean(&cleaned)?;
    let unit = indent_unit(text);
    // The deepest existing object along the path.
    let mut existing = 0;
    let mut node = &root;
    for segment in path {
        if node.members().is_none() {
            return Err(format!("'{}' is not an object", path[..existing].join(".")));
        }
        match node.get(segment) {
            Some(child) => {
                node = child;
                existing += 1;
            }
            None => break,
        }
    }
    let Some(members) = node.members() else {
        return Err(format!("'{}' is not an object", path[..existing].join(".")));
    };
    let created: Vec<String> = (existing..path.len())
        .map(|n| path[..=n].join("."))
        .collect();
    // What to put into `node`: the member, wrapped in the missing objects.
    let mut wrapped = value.clone();
    let mut member_key = key.to_string();
    if existing < path.len() {
        for segment in path[existing + 1..].iter().rev() {
            let mut object = serde_json::Map::new();
            object.insert(member_key, wrapped);
            wrapped = Value::Object(object);
            member_key = segment.to_string();
        }
        let mut object = serde_json::Map::new();
        object.insert(member_key, wrapped);
        wrapped = Value::Object(object);
        member_key = path[existing].to_string();
    } else if let Some(member) = members.iter().find(|m| m.key == key) {
        let old: Value = serde_json::from_str(
            cleaned
                .get(member.value.start..member.value.end)
                .ok_or("bad value span")?,
        )
        .map_err(|err| err.to_string())?;
        if old == *value {
            return Ok(None);
        }
        let rendered = render(value, unit.as_deref(), depth_of(path) + 1);
        let new = splice(text, member.value.start, member.value.end, &rendered)?;
        return checked(new).map(|text| Some((text, created)));
    }
    let depth = existing;
    let rendered = format!(
        "{}: {}",
        Value::from(member_key.as_str()),
        render(&wrapped, unit.as_deref(), depth + 1)
    );
    let new = insert_into(
        text,
        node,
        members.last().map(|m| m.value.end),
        &rendered,
        unit.as_deref(),
        depth,
    )?;
    checked(new).map(|text| Some((text, created)))
}

/// Inserts `item` (a member or an element, already rendered) into the container `node` at
/// nesting `depth`, after the byte `last_end` of its last item (or into the empty container).
fn insert_into(
    text: &str,
    node: &Node,
    last_end: Option<usize>,
    item: &str,
    unit: Option<&str>,
    depth: usize,
) -> Result<String, String> {
    match (last_end, unit) {
        (Some(end), Some(unit)) => {
            let indent = unit.repeat(depth + 1);
            splice(text, end, end, &format!(",\n{indent}{item}"))
        }
        (Some(end), None) => splice(text, end, end, &format!(", {item}")),
        // An empty container holding a comment: the item goes right after the bracket, the
        // comment stays.
        (None, unit) if !blank(text.get(node.start + 1..node.end - 1).unwrap_or_default()) => {
            match unit {
                Some(unit) => {
                    let inner = unit.repeat(depth + 1);
                    splice(
                        text,
                        node.start + 1,
                        node.start + 1,
                        &format!("\n{inner}{item}"),
                    )
                }
                None => splice(text, node.start + 1, node.start + 1, item),
            }
        }
        (None, Some(unit)) => {
            let inner = unit.repeat(depth + 1);
            let outer = unit.repeat(depth);
            splice(
                text,
                node.start + 1,
                node.end - 1,
                &format!("\n{inner}{item}\n{outer}"),
            )
        }
        (None, None) => splice(text, node.start + 1, node.end - 1, item),
    }
}

/// Appends `value` to the array at `path`, creating the array (and the objects before it) when
/// missing. Returns the new text and the dotted paths of the containers it created.
pub fn append_element(
    text: &str,
    path: &[&str],
    value: &Value,
) -> Result<(String, Vec<String>), String> {
    let Some((last, parent_path)) = path.split_last() else {
        return Err("an array path needs at least one key".to_string());
    };
    let root = parse(text)?;
    match locate(&root, path) {
        Some(array) => {
            let Some(elements) = array.elements() else {
                return Err(format!("'{}' is not an array", path.join(".")));
            };
            let unit = indent_unit(text);
            let rendered = render(value, unit.as_deref(), path.len() + 1);
            let new = insert_into(
                text,
                array,
                elements.last().map(|e| e.end),
                &rendered,
                unit.as_deref(),
                path.len(),
            )?;
            checked(new).map(|text| (text, Vec::new()))
        }
        None => {
            let array = Value::Array(vec![value.clone()]);
            match set_member(text, parent_path, last, &array)? {
                Some((text, mut created)) => {
                    created.push(path.join("."));
                    Ok((text, created))
                }
                None => Err("cannot create the array".to_string()),
            }
        }
    }
}

/// Removes the item `index` of the container `node` (member or element spans given as
/// `(start, end)` of each item).
fn remove_item(
    text: &str,
    node: &Node,
    items: &[(usize, usize)],
    index: usize,
) -> Result<String, String> {
    let Some(&(start, end)) = items.get(index) else {
        return Err("no such item".to_string());
    };
    if items.len() == 1 {
        let before = text.get(node.start + 1..start).unwrap_or_default();
        let after = text.get(end..node.end - 1).unwrap_or_default();
        if blank(before) && blank(after) {
            return splice(text, node.start + 1, node.end - 1, "");
        }
        // A comment beside the only item stays: the item goes with the blanks before it (and
        // its trailing comma).
        let from = node.start + 1 + before.trim_end_matches([' ', '\t', '\n', '\r']).len();
        let mut to = end;
        let rest = text.get(end..).unwrap_or_default();
        let gap = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        if rest[gap..].starts_with(',') {
            to = end + gap + 1;
        }
        return splice(text, from, to, "");
    }
    if index + 1 == items.len() {
        // The last item: from the end of the previous one (its comma goes too).
        let previous_end = items[index - 1].1;
        return splice(text, previous_end, end, "");
    }
    // Any other item: up to the start of the next one.
    splice(text, start, items[index + 1].0, "")
}

/// Removes the member `key` of the object at `path`. Returns `None` when there is none.
pub fn remove_member(text: &str, path: &[&str], key: &str) -> Result<Option<String>, String> {
    let root = parse(text)?;
    let Some(object) = locate(&root, path) else {
        return Ok(None);
    };
    let Some(members) = object.members() else {
        return Ok(None);
    };
    let Some(index) = members.iter().position(|m| m.key == key) else {
        return Ok(None);
    };
    let spans: Vec<(usize, usize)> = members.iter().map(|m| (m.key_start, m.value.end)).collect();
    checked(remove_item(text, object, &spans, index)?).map(Some)
}

/// Removes every element of the array at `path` for which `ours` holds. Returns `None` when
/// none matched.
pub fn remove_elements(
    text: &str,
    path: &[&str],
    ours: &dyn Fn(&Value) -> bool,
) -> Result<Option<String>, String> {
    let mut text = text.to_string();
    let mut changed = false;
    loop {
        let cleaned = clean(&text);
        let root = parse_clean(&cleaned)?;
        let Some(array) = locate(&root, path) else {
            break;
        };
        let Some(elements) = array.elements() else {
            break;
        };
        let found = elements.iter().position(|element| {
            cleaned
                .get(element.start..element.end)
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .is_some_and(|value| ours(&value))
        });
        let Some(index) = found else {
            break;
        };
        let spans: Vec<(usize, usize)> = elements.iter().map(|e| (e.start, e.end)).collect();
        text = checked(remove_item(&text, array, &spans, index)?)?;
        changed = true;
    }
    Ok(changed.then_some(text))
}

/// Whether the container at `path` exists and is empty.
pub fn is_empty_container(text: &str, path: &[&str]) -> bool {
    parse(text).ok().is_some_and(|root| {
        locate(&root, path).is_some_and(|node| match &node.kind {
            Kind::Object(members) => members.is_empty(),
            Kind::Array(elements) => elements.is_empty(),
            Kind::Scalar => false,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn insert_then_remove_gives_the_original_bytes() {
        for original in [
            "{\n  \"numStartups\": 3,\n  \"mcpServers\": {\n    \"other\": {\"command\": \"x\"}\n  }\n}\n",
            "{\n  \"mcpServers\": {}\n}\n",
            "{\"theme\":\"dark\",\"mcpServers\":{\"a\":1}}",
            "{}",
            "{\n\t\"x\": [1, 2]\n}",
        ] {
            let (inserted, _) = set_member(
                original,
                &["mcpServers"],
                "graphersal-memory",
                &json!({"command": "/bin/gm", "args": ["mcp"]}),
            )
            .unwrap()
            .unwrap();
            let value: Value = serde_json::from_str(&inserted).unwrap();
            assert_eq!(value["mcpServers"]["graphersal-memory"]["args"][0], "mcp");
            let removed = remove_member(&inserted, &["mcpServers"], "graphersal-memory")
                .unwrap()
                .unwrap();
            if original.contains("mcpServers") {
                assert_eq!(removed, original);
            } else {
                assert!(is_empty_container(&removed, &["mcpServers"]));
                let back = remove_member(&removed, &[], "mcpServers").unwrap().unwrap();
                assert_eq!(back, original);
            }
        }
    }

    #[test]
    fn the_style_of_the_file_is_kept() {
        let original = "{\n    \"a\": 1\n}\n";
        let (text, created) = set_member(original, &["mcpServers"], "x", &json!({"k": [1]}))
            .unwrap()
            .unwrap();
        assert_eq!(created, vec!["mcpServers".to_string()]);
        assert_eq!(
            text,
            "{\n    \"a\": 1,\n    \"mcpServers\": {\n        \"x\": {\n            \"k\": [\n                1\n            ]\n        }\n    }\n}\n"
        );
        let one_line = "{\"a\": 1}";
        let (text, _) = set_member(one_line, &[], "b", &json!({"c": 2}))
            .unwrap()
            .unwrap();
        assert_eq!(text, "{\"a\": 1, \"b\": {\"c\": 2}}");
    }

    #[test]
    fn an_unchanged_value_is_no_edit_and_a_changed_one_is_replaced() {
        let text = "{\"m\": {\"k\": {\"a\": 1}}}";
        assert!(
            set_member(text, &["m"], "k", &json!({"a": 1}))
                .unwrap()
                .is_none()
        );
        let (new, _) = set_member(text, &["m"], "k", &json!({"a": 2}))
            .unwrap()
            .unwrap();
        assert_eq!(new, "{\"m\": {\"k\": {\"a\": 2}}}");
    }

    #[test]
    fn arrays_append_and_remove_matching_elements() {
        let original =
            "{\n  \"hooks\": {\n    \"SessionStart\": [\n      {\"mine\": true}\n    ]\n  }\n}\n";
        let (text, created) =
            append_element(original, &["hooks", "SessionStart"], &json!({"ours": 1})).unwrap();
        assert!(created.is_empty());
        let (text, created) =
            append_element(&text, &["hooks", "SessionEnd"], &json!({"ours": 2})).unwrap();
        assert_eq!(created, vec!["hooks.SessionEnd".to_string()]);
        let ours = |v: &Value| v.get("ours").is_some();
        let text = remove_elements(&text, &["hooks", "SessionStart"], &ours)
            .unwrap()
            .unwrap();
        let text = remove_elements(&text, &["hooks", "SessionEnd"], &ours)
            .unwrap()
            .unwrap();
        assert!(is_empty_container(&text, &["hooks", "SessionEnd"]));
        let text = remove_member(&text, &["hooks"], "SessionEnd")
            .unwrap()
            .unwrap();
        assert_eq!(text, original);
        assert!(
            remove_elements(&text, &["hooks", "SessionStart"], &ours)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_middle_member_is_removed_with_its_comma() {
        let text = "{\"a\": 1, \"b\": 2, \"c\": 3}";
        assert_eq!(
            remove_member(text, &[], "b").unwrap().unwrap(),
            "{\"a\": 1, \"c\": 3}"
        );
        assert_eq!(
            remove_member(text, &[], "a").unwrap().unwrap(),
            "{\"b\": 2, \"c\": 3}"
        );
        assert!(remove_member(text, &[], "zz").unwrap().is_none());
    }

    #[test]
    fn invalid_text_and_wrong_types_are_errors() {
        assert!(parse("{\"a\": }").is_err());
        assert!(parse("{\"a\": 1} x").is_err());
        assert!(set_member("{\"m\": 3}", &["m"], "k", &json!(1)).is_err());
        assert!(append_element("{\"h\": {\"S\": 1}}", &["h", "S"], &json!(1)).is_err());
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        assert!(parse(&deep).is_err());
    }

    #[test]
    fn keys_with_escapes_and_strings_with_braces_are_located() {
        let text = "{\"a\\\"b\": \"}{,\", \"m\": {}}";
        let root = parse(text).unwrap();
        assert!(root.get("a\"b").is_some());
        let (new, _) = set_member(text, &["m"], "k", &json!(true))
            .unwrap()
            .unwrap();
        assert_eq!(new, "{\"a\\\"b\": \"}{,\", \"m\": {\"k\": true}}");
    }
    #[test]
    fn jsonc_comments_and_trailing_commas_stay_and_round_trip() {
        let original = "{\n  // my servers\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"mcp\": {\n    /* keep */ \"other\": {\"type\": \"local\", \"command\": [\"x\"],},\n  },\n}\n";
        assert!(value_of(original).is_ok());
        assert_eq!(clean(original).len(), original.len());
        let entry = json!({"type": "local", "command": ["/bin/gm", "mcp"], "enabled": true});
        let (inserted, created) = set_member(original, &["mcp"], "graphersal-memory", &entry)
            .unwrap()
            .unwrap();
        assert!(created.is_empty());
        assert!(inserted.contains("// my servers") && inserted.contains("/* keep */"));
        assert_eq!(
            value_of(&inserted).unwrap()["mcp"]["graphersal-memory"],
            entry
        );
        let removed = remove_member(&inserted, &["mcp"], "graphersal-memory")
            .unwrap()
            .unwrap();
        assert_eq!(removed, original);
        // A missing object is created and removed again.
        let (with_mcp, created) =
            set_member("{\n  // only a comment\n}\n", &["mcp"], "k", &json!(1))
                .unwrap()
                .unwrap();
        assert_eq!(created, vec!["mcp".to_string()]);
        assert!(with_mcp.contains("// only a comment"));
        let back = remove_member(&with_mcp, &[], "mcp").unwrap().unwrap();
        assert!(back.contains("// only a comment"));
        assert!(value_of(&back).unwrap().as_object().unwrap().is_empty());
    }

    #[test]
    fn strings_that_look_like_comments_are_kept() {
        let text = "{\"url\": \"http://x/*y*/\", \"a\": [1,]}";
        let cleaned = clean(text);
        assert!(cleaned.contains("http://x/*y*/"));
        assert_eq!(value_of(text).unwrap()["a"], json!([1]));
        assert_eq!(clean("{\"a\": 1}"), "{\"a\": 1}");
    }
}
