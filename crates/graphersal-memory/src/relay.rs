//! The browser UI's relay: `graphersal-memory ui` (the HTTP server on loopback, the network
//! boundary) hands each `/api/` request it does not answer itself to the daemon, one request per
//! socket connection, and the daemon answers it with `graphersal-serve`'s `Server::handle` on a
//! context of the store in allow-list mode (`crate::mcp::StoreRuntime::relay`).
//!
//! The frame (internal, like the rest of `crate::protocol`):
//!
//! - the client's opening line: `{"protocol", "relay": "http", "shim_version", "port", "method",
//!   "target", "headers": [[name, value], ...], "body_length"}`, then exactly `body_length`
//!   bytes of body. `port` is the `ui` process's port (the relay context's `HostCheck` checks
//!   the forwarded `Host` and `Origin` against it), `target` the request target (path and
//!   query), `headers` only the ones the API reads ([`FORWARDED_HEADERS`]: never the token);
//! - the daemon's answer line: `{"ok": true, "status", "headers": [[name, value], ...],
//!   "body_length"}`, then exactly `body_length` bytes; or a refusal line
//!   (`crate::protocol::refusal`) and nothing else.
//!
//! Bodies are limited to graphersal-serve's `MAX_BODY_BYTES` both ways.

use std::io::{BufRead, BufReader, Read, Write};
use std::time::Duration;

use serde_json::{Value, json};

use crate::protocol::{PROTOCOL, VERSION};

/// The `relay` value of the opening line.
pub const RELAY_HTTP: &str = "http";

/// The request headers the `ui` process forwards (lowercase); everything else stays in it (the
/// `Authorization` token above all).
pub const FORWARDED_HEADERS: [&str; 5] = [
    "host",
    "origin",
    "content-type",
    "accept",
    "x-graphersal-client",
];

/// The largest body relayed either way.
pub const MAX_BODY_BYTES: usize = graphersal_serve::MAX_BODY_BYTES;

/// How long the client waits for the daemon's answer (a query on a large store, the store
/// opening on first use).
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);

/// The saved queries the relay context answers: the memory functions and the browser UI's own
/// (`crate::catalog`), never an internal one (session indexing, merge, export, migrations).
pub fn selection() -> graphersal_session::queries::SavedQuerySelection {
    let names = crate::catalog::FUNCTIONS
        .iter()
        .chain(crate::catalog::UI_FUNCTIONS.iter())
        .copied();
    graphersal_session::queries::SavedQuerySelection::of(Vec::<String>::new(), names)
        .excluding(crate::catalog::INTERNAL_FUNCTIONS)
}

/// One relayed request as the daemon reads it from the opening line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The client's version (diagnostics).
    pub shim_version: String,
    /// The `ui` process's port.
    pub port: u16,
    /// The HTTP method.
    pub method: String,
    /// The request target (path and query).
    pub target: String,
    /// The forwarded headers.
    pub headers: Vec<(String, String)>,
    /// The body's length in bytes (it follows the line).
    pub body_length: usize,
}

/// The frame of an opening line whose `relay` key is set; the error names what is wrong.
pub fn parse(object: &serde_json::Map<String, Value>) -> Result<Frame, String> {
    let relay = object.get("relay").and_then(Value::as_str);
    if relay != Some(RELAY_HTTP) {
        return Err(format!(
            "unknown relay {}: expected \"{RELAY_HTTP}\"",
            object.get("relay").unwrap_or(&Value::Null)
        ));
    }
    let text = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("the relay line has no {key}"))
    };
    let port = object
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0)
        .ok_or("the relay line has no port")?;
    let body_length = object
        .get("body_length")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or("the relay line has no body_length")?;
    if body_length > MAX_BODY_BYTES {
        return Err(format!(
            "the relayed body has {body_length} bytes, more than {MAX_BODY_BYTES}"
        ));
    }
    let mut headers = Vec::new();
    for pair in object
        .get("headers")
        .and_then(Value::as_array)
        .ok_or("the relay line has no headers")?
    {
        match pair.as_array().map(Vec::as_slice) {
            Some([name, value]) => match (name.as_str(), value.as_str()) {
                (Some(name), Some(value)) => headers.push((name.to_string(), value.to_string())),
                _ => return Err("a relayed header is not two texts".into()),
            },
            _ => return Err("a relayed header is not a [name, value] pair".into()),
        }
    }
    let target = text("target")?;
    if !target.starts_with("/api/") {
        return Err(format!("the relay serves /api/ only, not {target}"));
    }
    Ok(Frame {
        shim_version: text("shim_version").unwrap_or_default(),
        port,
        method: text("method")?,
        target,
        headers,
        body_length,
    })
}

/// The opening line of a relayed request (without its line end).
pub fn opening_line(
    port: u16,
    method: &str,
    target: &str,
    headers: &[(String, String)],
    body_length: usize,
) -> String {
    let headers: Vec<Value> = headers
        .iter()
        .filter(|(name, _)| FORWARDED_HEADERS.contains(&name.to_ascii_lowercase().as_str()))
        .map(|(name, value)| json!([name, value]))
        .collect();
    json!({
        "protocol": PROTOCOL, "relay": RELAY_HTTP, "shim_version": VERSION, "port": port,
        "method": method, "target": target, "headers": headers, "body_length": body_length,
    })
    .to_string()
}

/// The answer line of a relayed request (without its line end).
pub fn answer_line(status: u16, headers: &[(String, String)], body_length: usize) -> String {
    let headers: Vec<Value> = headers
        .iter()
        .map(|(name, value)| json!([name, value]))
        .collect();
    json!({"ok": true, "status": status, "headers": headers, "body_length": body_length})
        .to_string()
}

/// Reads exactly `length` bytes of body.
pub fn read_body(reader: &mut impl Read, length: usize) -> Result<Vec<u8>, String> {
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|err| format!("the relayed body ended early: {err}"))?;
    Ok(body)
}

/// A relayed answer: status, headers, body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The HTTP status.
    pub status: u16,
    /// The answer's headers.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

/// Relays one request to the daemon at `location` (started when none answers, as after a
/// daemon restart: `crate::shim::connect_again`) and reads its answer. A refusal of the daemon
/// is an error with its message.
pub fn send(
    location: &crate::shim::DaemonLocation,
    port: u16,
    method: &str,
    target: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<Answer, String> {
    let log = location.run_dir.join(crate::ipc::LOG_FILE);
    let failed = |what: String| format!("the daemon failed: {what}; see {}", log.display());
    let stream = crate::shim::connect_again(location)?;
    let mut writer = stream
        .try_clone()
        .map_err(|err| format!("cannot use the daemon's socket: {err}"))?;
    let line = opening_line(port, method, target, headers, body.len());
    writer
        .write_all(line.as_bytes())
        .and_then(|()| writer.write_all(b"\n"))
        .and_then(|()| writer.write_all(body))
        .and_then(|()| writer.flush())
        .map_err(|err| failed(format!("cannot send it the request ({err})")))?;
    let _ = stream.set_read_timeout(Some(ANSWER_TIMEOUT));
    let mut reader = BufReader::new(stream);
    let mut answer = Vec::new();
    let limit = u64::try_from(crate::protocol::MAX_HANDSHAKE_BYTES).unwrap_or(u64::MAX);
    (&mut reader)
        .take(limit)
        .read_until(b'\n', &mut answer)
        .map_err(|err| failed(format!("no answer ({err})")))?;
    if answer.last() != Some(&b'\n') {
        return Err(failed("its answer ended before the end of the line".into()));
    }
    let value: Value =
        serde_json::from_slice(&answer).map_err(|_| failed("its answer is not JSON".into()))?;
    if value["ok"] != true {
        return Err(value["error"]["message"]
            .as_str()
            .unwrap_or("the daemon refused the request")
            .to_string());
    }
    let status = value["status"]
        .as_u64()
        .and_then(|status| u16::try_from(status).ok())
        .ok_or_else(|| failed("its answer has no status".into()))?;
    let length = value["body_length"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n <= MAX_BODY_BYTES)
        .ok_or_else(|| failed("its answer has no body length".into()))?;
    let headers = value["headers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pair| {
            Some((
                pair.get(0)?.as_str()?.to_string(),
                pair.get(1)?.as_str()?.to_string(),
            ))
        })
        .collect();
    let body = read_body(&mut reader, length).map_err(failed)?;
    Ok(Answer {
        status,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(line: &str) -> serde_json::Map<String, Value> {
        serde_json::from_str::<Value>(line)
            .unwrap()
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn a_relay_line_round_trips_without_the_token() {
        let headers = vec![
            ("Host".to_string(), "127.0.0.1:4100".to_string()),
            ("Authorization".to_string(), "Bearer secret".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Cookie".to_string(), "x=1".to_string()),
        ];
        let line = opening_line(4100, "POST", "/api/query", &headers, 12);
        assert!(
            !line.contains("secret") && !line.contains("Cookie"),
            "{line}"
        );
        let frame = parse(&object(&line)).unwrap();
        assert_eq!(frame.port, 4100);
        assert_eq!(frame.method, "POST");
        assert_eq!(frame.target, "/api/query");
        assert_eq!(frame.body_length, 12);
        assert_eq!(frame.shim_version, VERSION);
        assert_eq!(
            frame.headers,
            vec![
                ("Host".to_string(), "127.0.0.1:4100".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ]
        );
    }

    #[test]
    fn bad_relay_lines_are_refused_with_a_reason() {
        let base = opening_line(4100, "GET", "/api/info", &[], 0);
        let mut value: Value = serde_json::from_str(&base).unwrap();
        value["target"] = json!("/index.html");
        assert!(
            parse(value.as_object().unwrap())
                .unwrap_err()
                .contains("/api/ only")
        );
        let mut value: Value = serde_json::from_str(&base).unwrap();
        value["body_length"] = json!(MAX_BODY_BYTES + 1);
        assert!(
            parse(value.as_object().unwrap())
                .unwrap_err()
                .contains("more than")
        );
        let mut value: Value = serde_json::from_str(&base).unwrap();
        value["port"] = json!(0);
        assert!(
            parse(value.as_object().unwrap())
                .unwrap_err()
                .contains("port")
        );
        let mut value: Value = serde_json::from_str(&base).unwrap();
        value["relay"] = json!("ftp");
        assert!(
            parse(value.as_object().unwrap())
                .unwrap_err()
                .contains("unknown relay")
        );
        let answer = answer_line(200, &[("A".into(), "b".into())], 3);
        let value: Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(value["status"], 200);
        assert_eq!(value["headers"], json!([["A", "b"]]));
    }
}
