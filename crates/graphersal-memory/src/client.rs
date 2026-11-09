//! A small MCP client for the product's own use of the memory functions (`import`, the hooks):
//! newline-delimited JSON-RPC over any reader and writer. In the product it talks to the daemon
//! through a `use` connection (`connect`), so the store keeps its single writer; tests run it
//! against a store served in the test process.

use std::io::{BufRead, Write};
use std::time::Duration;

use serde_json::{Value, json};

use crate::cli::Profile;
use crate::ipc::Stream;
use crate::shim::{self, DaemonLocation, ShimOptions};

/// An initialized MCP client of one connection.
pub struct McpClient<R, W> {
    from: R,
    to: W,
    next_id: u64,
}

impl<R: BufRead, W: Write> McpClient<R, W> {
    /// Initializes an MCP session over `from`/`to`.
    pub fn start(from: R, to: W) -> Result<Self, String> {
        let mut client = Self {
            from,
            to,
            next_id: 1,
        };
        let init = client.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "graphersal-memory", "version": crate::protocol::VERSION}}),
        )?;
        if !init["result"].is_object() {
            return Err(format!("the memory server refused to start: {init}"));
        }
        client.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(client)
    }

    fn send(&mut self, message: &Value) -> Result<(), String> {
        writeln!(self.to, "{message}")
            .and_then(|()| self.to.flush())
            .map_err(|err| format!("cannot talk to the memory server: {err}"))
    }

    /// One request and its answer (notifications in between are skipped).
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        loop {
            let mut line = String::new();
            let n = self
                .from
                .read_line(&mut line)
                .map_err(|err| format!("the memory server did not answer: {err}"))?;
            if n == 0 {
                return Err("the memory server closed the connection".to_string());
            }
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message["id"] == json!(id) {
                return Ok(message);
            }
        }
    }

    /// Calls the tool `name`: its answer as JSON, or the error the tool reported.
    pub fn call(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        let answer = self.request("tools/call", json!({"name": name, "arguments": arguments}))?;
        if let Some(error) = answer.get("error") {
            return Err(format!(
                "{name}: {}",
                error["message"].as_str().unwrap_or("refused")
            ));
        }
        let result = &answer["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        if result["isError"] == Value::Bool(true) {
            return Err(format!("{name}: {}", text.trim()));
        }
        answer_json(text).ok_or_else(|| format!("{name}: unexpected answer: {}", short(text)))
    }
}

/// The JSON of a tool's text answer (the whole text, or its last line that is JSON: the
/// engine may put a header line first).
fn answer_json(text: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    text.lines()
        .rev()
        .find_map(|line| serde_json::from_str(line.trim()).ok())
}

fn short(text: &str) -> String {
    text.chars().take(200).collect()
}

/// A client of the daemon's `use` server bound to `project` (the daemon starts when none
/// runs). Every answer must arrive within `timeout`.
pub fn connect(
    location: &DaemonLocation,
    project: &str,
    timeout: Duration,
) -> Result<McpClient<std::io::BufReader<Stream>, Stream>, String> {
    let options = ShimOptions {
        location: location.clone(),
        profile: Profile::Use,
        project: project.to_string(),
        harness: "graphersal-memory".to_string(),
    };
    let (stream, reader) = shim::open(&options)?;
    let _ = reader.get_ref().set_read_timeout(Some(timeout));
    McpClient::start(reader, stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_found_after_a_header() {
        assert_eq!(answer_json("{\"a\":1}"), Some(json!({"a": 1})));
        assert_eq!(
            answer_json("call: x\nok: 1 result\n[1,2]"),
            Some(json!([1, 2]))
        );
        assert_eq!(answer_json("no json"), None);
    }
}
