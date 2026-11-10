//! Minimal-change edits of a TOML file (Codex's `config.toml`): reading goes through the `toml`
//! crate, writing is a text edit that keeps every byte of the user's file except the part
//! changed.
//!
//! Two edits exist:
//!
//! - a **managed block** of whole tables between `# graphersal-memory:begin fingerprint=...`
//!   and `# graphersal-memory:end` comment lines. It is inserted before the first table header
//!   of the file (so the top-level keys above stay top-level, and a key someone appends at the
//!   end of the file still lands in the table it landed in before), or at the end of a file
//!   without tables. The fingerprint on the begin line is that of the block's text, so a hand
//!   edit inside is detected (and reported, never overwritten), as in the Markdown rule blocks;
//! - one **boolean value** (`[features]` `memories = true`, or `features.memories = true` at the
//!   top level) replaced in place.
//!
//! Every edit is checked by parsing the result: the document must equal the one before with
//! exactly the intended change, else the edit is refused (the file is left as it is).

use serde_json::Value;

use crate::fsutil;

/// The first characters of the managed block's begin line.
pub const BEGIN: &str = "# graphersal-memory:begin";

/// The managed block's end line.
pub const END: &str = "# graphersal-memory:end";

/// The document of a TOML text, as JSON (tables become objects).
pub fn parse(text: &str) -> Result<Value, String> {
    let table: toml::Table = toml::from_str(text).map_err(|err| err.message().to_string())?;
    serde_json::to_value(table).map_err(|err| err.to_string())
}

/// The managed block for `body` (whole TOML tables), marker lines included, ending with a
/// newline.
pub fn wrap(body: &str) -> String {
    format!(
        "{BEGIN} fingerprint={} (managed by `graphersal-memory install`; `graphersal-memory \
         uninstall` removes it; an edit inside is reported as a conflict)\n{body}{END}\n",
        fsutil::fingerprint(body.as_bytes())
    )
}

/// A managed block found in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Its first byte (the begin line).
    pub start: usize,
    /// One past its last byte (after the end line's newline).
    pub end: usize,
    /// Whether its text still matches the fingerprint on its begin line.
    pub intact: bool,
    /// Its text between the marker lines.
    pub body: String,
}

/// The managed block of `text`: `Ok(None)` without one, `Err` for a begin line without an end
/// line.
pub fn find(text: &str) -> Result<Option<Found>, String> {
    let Some(start) = text
        .match_indices(BEGIN)
        .map(|(at, _)| at)
        .find(|at| *at == 0 || text[..*at].ends_with('\n'))
    else {
        return Ok(None);
    };
    let line_end = text[start..]
        .find('\n')
        .map(|n| start + n + 1)
        .ok_or("the managed block has no end marker")?;
    let recorded = text[start..line_end]
        .split_whitespace()
        .find_map(|word| word.strip_prefix("fingerprint="))
        .unwrap_or_default()
        .to_string();
    let end_at = text[line_end..]
        .match_indices(END)
        .map(|(at, _)| line_end + at)
        .find(|at| text[..*at].ends_with('\n'))
        .ok_or("the managed block has no end marker")?;
    let body = text[line_end..end_at].to_string();
    let mut end = end_at + END.len();
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
    }))
}

/// The byte offset of the first table header (`[table]` or `[[array]]`) of `text` outside a
/// multi-line string, or `None` when it has none.
fn first_header(text: &str) -> Option<usize> {
    let mut at = 0;
    let mut in_string: Option<&str> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        match in_string {
            Some(quote) => {
                if line.matches(quote).count() % 2 == 1 {
                    in_string = None;
                }
            }
            None => {
                if trimmed.starts_with('[') {
                    return Some(at);
                }
                for quote in ["\"\"\"", "'''"] {
                    if line.matches(quote).count() % 2 == 1 {
                        in_string = Some(quote);
                        break;
                    }
                }
            }
        }
        at += line.len();
    }
    None
}

/// `text` with the managed block of `body` written: replaced where a block is, else inserted
/// before the first table header (a blank line after it), else appended (a blank line before
/// it). `Err` when the result does not parse.
pub fn put_block(text: &str, body: &str) -> Result<String, String> {
    let block = wrap(body);
    let new = match find(text)? {
        Some(found) => format!("{}{block}{}", &text[..found.start], &text[found.end..]),
        None => match first_header(text) {
            Some(at) => format!("{}{block}\n{}", &text[..at], &text[at..]),
            None => {
                let sep = if text.is_empty() || text.ends_with("\n\n") {
                    ""
                } else if text.ends_with('\n') {
                    "\n"
                } else {
                    "\n\n"
                };
                format!("{text}{sep}{block}")
            }
        },
    };
    parse(&new).map_err(|err| format!("the edit would not be valid TOML ({err})"))?;
    Ok(new)
}

/// `text` without its managed block (and the blank line install put beside it), or `None`
/// when it has none.
pub fn remove_block(text: &str) -> Result<Option<String>, String> {
    let Some(found) = find(text)? else {
        return Ok(None);
    };
    let head = &text[..found.start];
    let tail = &text[found.end..];
    let new = if let Some(rest) = tail.strip_prefix('\n') {
        // Inserted before a table header: the blank line after it.
        format!("{head}{rest}")
    } else if let Some(rest) = tail.strip_prefix("\r\n") {
        format!("{head}{rest}")
    } else if tail.is_empty() {
        // Appended: the blank line before it.
        let head = head
            .strip_suffix("\n\n")
            .map(|h| format!("{h}\n"))
            .unwrap_or_else(|| head.to_string());
        head.to_string()
    } else {
        format!("{head}{tail}")
    };
    Ok(Some(new))
}

/// `text` without the table `[<header>]` written as a header line of its own (its key-value
/// lines up to the next header or the end), or `None` when there is no such line. Used for
/// our tables found outside the managed block (when its marker comments were lost).
pub fn remove_table(text: &str, header: &str) -> Option<String> {
    let mut at = 0;
    let mut start = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if start.is_some() {
                return start.map(|s: usize| format!("{}{}", &text[..s], &text[at..]));
            }
            let name = trimmed
                .split('#')
                .next()
                .unwrap_or_default()
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim();
            if name == header {
                start = Some(at);
            }
        }
        at += line.len();
    }
    start.map(|s| text[..s].to_string())
}

/// A TOML basic string.
pub fn string(text: &str) -> String {
    Value::from(text).to_string()
}

/// A TOML array of basic strings.
pub fn strings(items: &[String]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|item| string(item))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Whether `line` sets `key` (bare or quoted), with the value after the `=`.
fn assignment<'a>(line: &'a str, key: &str) -> Option<(usize, &'a str)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    let rest = [key.to_string(), string(key), format!("'{key}'")]
        .iter()
        .find_map(|form| trimmed.strip_prefix(form.as_str()))?;
    let after_key = rest.trim_start();
    let value = after_key.strip_prefix('=')?;
    let value_start = indent + (trimmed.len() - value.len());
    Some((value_start, value))
}

/// `text` with the boolean `table.key` set to `value` in place (`[table]` with `key = ...`, or
/// `table.key = ...` at the top level), or `None` when it is not written as a plain boolean
/// there (inline tables, a profile file, ...). The result is checked: only that value differs.
pub fn set_bool(text: &str, table: &str, key: &str, value: bool) -> Result<Option<String>, String> {
    let before = parse(text)?;
    if before[table][key].as_bool().is_none() {
        return Ok(None);
    }
    let mut current_table = String::new();
    let mut at = 0;
    let mut in_string: Option<&str> = None;
    let dotted = format!("{table}.{key}");
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if let Some(quote) = in_string {
            if line.matches(quote).count() % 2 == 1 {
                in_string = None;
            }
            at += line.len();
            continue;
        }
        if trimmed.starts_with('[') {
            current_table = trimmed
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
        } else {
            let found = if current_table == table {
                assignment(line, key)
            } else if current_table.is_empty() {
                assignment(line, &dotted)
            } else {
                None
            };
            if let Some((value_start, rest)) = found {
                let lead = rest.len() - rest.trim_start().len();
                let token_start = at + value_start + lead;
                let old = if rest.trim_start().starts_with("true") {
                    "true"
                } else if rest.trim_start().starts_with("false") {
                    "false"
                } else {
                    return Ok(None);
                };
                let new = format!(
                    "{}{}{}",
                    &text[..token_start],
                    if value { "true" } else { "false" },
                    &text[token_start + old.len()..]
                );
                let mut expected = before.clone();
                expected[table][key] = Value::Bool(value);
                return Ok((parse(&new).ok() == Some(expected)).then_some(new));
            }
            for quote in ["\"\"\"", "'''"] {
                if line.matches(quote).count() % 2 == 1 {
                    in_string = Some(quote);
                    break;
                }
            }
        }
        at += line.len();
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "[mcp_servers.x]\ncommand = \"/x\"\nargs = [\"mcp\"]\n";

    #[test]
    fn a_block_goes_before_the_first_table_and_comes_out_byte_for_byte() {
        let original =
            "# my config\nmodel = \"o3\"\n\n[projects.\"/w\"]\ntrust_level = \"trusted\"\n";
        let new = put_block(original, BODY).unwrap();
        assert!(new.starts_with("# my config\nmodel = \"o3\"\n\n# graphersal-memory:begin"));
        assert!(new.ends_with(
            "# graphersal-memory:end\n\n[projects.\"/w\"]\ntrust_level = \"trusted\"\n"
        ));
        let doc = parse(&new).unwrap();
        assert_eq!(doc["model"], "o3");
        assert_eq!(doc["mcp_servers"]["x"]["command"], "/x");
        assert_eq!(doc["projects"]["/w"]["trust_level"], "trusted");
        let found = find(&new).unwrap().unwrap();
        assert!(found.intact);
        assert_eq!(found.body, BODY);
        assert_eq!(remove_block(&new).unwrap().unwrap(), original);
        // Written again: replaced in place.
        assert_eq!(put_block(&new, BODY).unwrap(), new);
    }

    #[test]
    fn a_file_without_tables_gets_the_block_appended() {
        for original in [
            "",
            "model = \"o3\"\n",
            "model = \"o3\"",
            "model = \"o3\"\n\n",
        ] {
            let new = put_block(original, BODY).unwrap();
            assert!(new.ends_with("# graphersal-memory:end\n"), "{new:?}");
            let back = remove_block(&new).unwrap().unwrap();
            let expected = if original.ends_with('\n') || original.is_empty() {
                original.to_string()
            } else {
                format!("{original}\n")
            };
            assert_eq!(back.trim_end(), expected.trim_end(), "{original:?}");
        }
        assert_eq!(remove_block("x = 1\n").unwrap(), None);
    }

    #[test]
    fn headers_inside_multi_line_strings_are_not_tables() {
        let original =
            "developer_instructions = \"\"\"\n[not a table]\n\"\"\"\n\n[features]\nx = true\n";
        let new = put_block(original, BODY).unwrap();
        let doc = parse(&new).unwrap();
        assert_eq!(doc["developer_instructions"], "[not a table]\n");
        assert_eq!(doc["features"]["x"], true);
        assert!(new.find(BEGIN).unwrap() > new.find("\"\"\"\n\n").unwrap());
    }

    #[test]
    fn a_hand_edit_inside_the_block_breaks_its_fingerprint() {
        let new = put_block("", BODY).unwrap();
        let edited = new.replace("/x", "/y");
        assert!(!find(&edited).unwrap().unwrap().intact);
        assert!(find("# graphersal-memory:begin x\n[a]\n").is_err());
    }

    #[test]
    fn a_table_outside_the_block_is_removed_by_its_header() {
        let text = "a = 1\n[mcp_servers.x]\ncommand = \"/x\"\n\n[other]\nb = 2\n";
        assert_eq!(
            remove_table(text, "mcp_servers.x").unwrap(),
            "a = 1\n[other]\nb = 2\n"
        );
        assert_eq!(
            remove_table("[mcp_servers.x]\nc = 1\n", "mcp_servers.x").unwrap(),
            ""
        );
        assert_eq!(remove_table(text, "mcp_servers.y"), None);
    }

    #[test]
    fn a_boolean_is_set_in_place_in_either_form() {
        let table = "model = \"o3\"\n\n[features]\n# keep\nmemories = true # mine\nhooks = true\n";
        let new = set_bool(table, "features", "memories", false)
            .unwrap()
            .unwrap();
        assert_eq!(
            new,
            "model = \"o3\"\n\n[features]\n# keep\nmemories = false # mine\nhooks = true\n"
        );
        assert_eq!(
            set_bool(&new, "features", "memories", true)
                .unwrap()
                .unwrap(),
            table
        );
        let dotted = "features.memories = true\n[x]\nmemories = true\n";
        assert_eq!(
            set_bool(dotted, "features", "memories", false)
                .unwrap()
                .unwrap(),
            "features.memories = false\n[x]\nmemories = true\n"
        );
        // Not there, or not a plain boolean line: nothing.
        assert_eq!(
            set_bool("x = 1\n", "features", "memories", false).unwrap(),
            None
        );
        assert_eq!(
            set_bool(
                "features = { memories = true }\n",
                "features",
                "memories",
                false
            )
            .unwrap(),
            None
        );
    }
}
