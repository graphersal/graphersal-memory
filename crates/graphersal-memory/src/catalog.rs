//! The product catalog: the memory schema and the memory functions (saved queries in folder
//! `memory`), built into the binary and applied to every store it serves.
//!
//! The definitions live beside this file: `catalog/schema.json` (the schema), `catalog/queries.json`
//! (each function's name, description and parameters) and `catalog/<name>.rhai` (its body). Every
//! saved query carries `meta` `{"origin": "product", "catalog": "<fingerprint>"}`: a store whose
//! product queries do not carry the fingerprint of this binary's catalog gets the schema and the
//! queries again (the product owns them; a local edit of a product query is replaced), and a
//! product query this binary no longer has is dropped.
//!
//! Project scoping: every function that reads or writes memories takes a `project` parameter
//! (the project key, `""` for none). The MCP layer binds it per connection and hides it from
//! the agent (`crate::mcp`); a vertex with a `project` property is visible only from that
//! project, one without it (global) from every project.

use graphersal::auth::{AccessPolicy, ActionKind, ResourceKind};
use graphersal_session::Session;
use serde_json::{Value, json};

/// The schema of a memory store.
pub const SCHEMA: &str = include_str!("catalog/schema.json");

/// Name, description and parameters of every memory function, in tool order.
const QUERIES: &str = include_str!("catalog/queries.json");

/// Name, description and parameters of the internal functions: the product's own writes (the
/// session indexing), never offered as tools of the `use` server.
const INTERNAL: &str = include_str!("catalog/internal.json");

/// The internal functions, in the order of `catalog/internal.json`.
pub const INTERNAL_FUNCTIONS: [&str; 4] = [
    "indexed_sessions",
    "index_session",
    "index_turns",
    "set_summary",
];

/// The folder of the product's saved queries.
pub const FOLDER: &str = "memory";

/// The memory functions, in the order of the `use` profile's tool list.
pub const FUNCTIONS: [&str; 7] = [
    "memories", "memory", "remember", "forget", "sessions", "session", "recall",
];

/// The functions that declare the `project` parameter, which each MCP connection binds to its
/// project (`graphersal_mcp::Profile::with_bound_params` binds it for every saved query that
/// declares it; a test keeps this list and the declarations in step).
#[cfg(test)]
pub const PROJECT_BOUND: [&str; 7] = [
    "memories", "memory", "remember", "forget", "sessions", "session", "recall",
];

/// The name of the parameter bound to the connection's project (`crate::mcp`).
pub const PROJECT_PARAM: &str = "project";

/// The body of the memory function `name`.
fn body(name: &str) -> Option<&'static str> {
    Some(match name {
        "memories" => include_str!("catalog/memories.rhai"),
        "memory" => include_str!("catalog/memory.rhai"),
        "remember" => include_str!("catalog/remember.rhai"),
        "forget" => include_str!("catalog/forget.rhai"),
        "sessions" => include_str!("catalog/sessions.rhai"),
        "session" => include_str!("catalog/session.rhai"),
        "recall" => include_str!("catalog/recall.rhai"),
        "indexed_sessions" => include_str!("catalog/indexed_sessions.rhai"),
        "index_session" => include_str!("catalog/index_session.rhai"),
        "index_turns" => include_str!("catalog/index_turns.rhai"),
        "set_summary" => include_str!("catalog/set_summary.rhai"),
        _ => return None,
    })
}

/// One memory function as a saved-query spec of `graphersal_session` (without `meta`).
#[derive(Debug, Clone)]
pub struct Function {
    /// The query name (= the tool name).
    pub name: String,
    /// The spec: name, folder, description, params, body.
    pub spec: Value,
}

/// The memory functions of this binary's catalog.
pub fn functions() -> Result<Vec<Function>, String> {
    parse_functions(QUERIES)
}

/// The internal functions of this binary's catalog.
pub fn internal_functions() -> Result<Vec<Function>, String> {
    parse_functions(INTERNAL)
}

fn parse_functions(text: &str) -> Result<Vec<Function>, String> {
    let list: Value = serde_json::from_str(text)
        .map_err(|err| format!("the built-in catalog is not valid JSON: {err}"))?;
    let entries = list
        .as_array()
        .ok_or("the built-in catalog is not a list of functions")?;
    let mut out = Vec::new();
    for entry in entries {
        let name = entry["name"]
            .as_str()
            .ok_or("a built-in function has no name")?
            .to_string();
        let body =
            body(&name).ok_or_else(|| format!("the built-in function {name} has no body"))?;
        out.push(Function {
            spec: json!({
                "name": name,
                "folder": FOLDER,
                "description": entry["description"],
                "params": entry["params"],
                "body": body,
            }),
            name,
        });
    }
    Ok(out)
}

/// A fingerprint of the whole catalog (schema, specs, bodies): FNV-1a 64, hex.
pub fn fingerprint() -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |text: &str| {
        for byte in text.bytes().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    feed(SCHEMA);
    feed(QUERIES);
    feed(INTERNAL);
    for name in FUNCTIONS.iter().chain(INTERNAL_FUNCTIONS.iter()) {
        feed(body(name).unwrap_or_default());
    }
    format!("{hash:016x}")
}

/// The `meta` text every product query carries.
fn meta() -> String {
    json!({"origin": "product", "catalog": fingerprint()}).to_string()
}

/// Whether a stored query's `meta` text marks it as the product's, and with which fingerprint.
fn product_fingerprint(meta: &Value) -> Option<String> {
    let parsed: Value = serde_json::from_str(meta.as_str()?).ok()?;
    if parsed["origin"] != "product" {
        return None;
    }
    Some(parsed["catalog"].as_str().unwrap_or_default().to_string())
}

/// What [`apply`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The store already had this catalog.
    Current,
    /// The schema and the queries were written (a new store, or another catalog version).
    Written,
}

/// Makes the served store hold this binary's catalog: the schema and every memory function,
/// written when any product query is missing or carries another fingerprint; product queries
/// this binary no longer has are dropped.
pub fn apply(session: &mut Session) -> Result<Applied, String> {
    let mut wanted = functions()?;
    wanted.extend(internal_functions()?);
    let current = fingerprint();
    let listed = session.list_queries(None).map_err(|err| err.to_string())?;
    let mut stored = Vec::new();
    for entry in listed.as_array().into_iter().flatten() {
        let Some(name) = entry["name"].as_str() else {
            continue;
        };
        let full = session.get_query(name).map_err(|err| err.to_string())?;
        stored.push((name.to_string(), product_fingerprint(&full["meta"])));
    }
    let up_to_date = wanted.iter().all(|function| {
        stored
            .iter()
            .any(|(name, print)| *name == function.name && print.as_deref() == Some(&current))
    }) && stored
        .iter()
        .all(|(name, print)| print.is_none() || wanted.iter().any(|f| f.name == *name));
    if up_to_date {
        return Ok(Applied::Current);
    }
    session
        .set_schema(SCHEMA, None)
        .map_err(|err| format!("the memory schema cannot be applied: {err}"))?;
    let meta = meta();
    for function in &wanted {
        let mut spec = function.spec.clone();
        spec["meta"] = json!(meta);
        if stored.iter().any(|(name, _)| *name == function.name) {
            spec["replaces"] = json!(function.name);
        }
        let answer = session.define_query(&spec);
        if answer["ok"] != true {
            return Err(format!(
                "the memory function {} cannot be saved: {}",
                function.name, answer["errors"]
            ));
        }
    }
    for (name, print) in &stored {
        if print.is_some() && !wanted.iter().any(|f| f.name == *name) {
            session.drop_query(name).map_err(|err| err.to_string())?;
        }
    }
    Ok(Applied::Written)
}

/// Runs the saved query `name` with the named arguments `args` on the session (the product's
/// own calls on the store's session thread: the session indexing) and returns its value. A
/// failing run changes nothing (one unit) and is an error.
pub fn call(session: &mut Session, name: &str, args: &Value) -> Result<Value, String> {
    let script = graphersal_session::queries::query_call_text(name, args)
        .map_err(|err| format!("{name}: {err}"))?;
    let options = graphersal_session::RunOptions {
        no_limit: true,
        ..Default::default()
    };
    let answer: Value = serde_json::from_str(&session.execute(&script, &options))
        .map_err(|err| format!("{name}: malformed engine answer: {err}"))?;
    let execution = &answer["execution"];
    if answer["ok"] != true {
        let error = &execution["error"];
        let message = error["diagnostic"]
            .as_str()
            .or_else(|| error["message"].as_str())
            .unwrap_or("the query failed");
        return Err(format!("{name}: {message}"));
    }
    // A single value comes back as the one result; a list as the results.
    let value = match (&execution["results"], answer["terminal"].as_str()) {
        (Value::Array(results), Some("value")) if results.len() == 1 => results[0].clone(),
        (other, _) => other.clone(),
    };
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(format!("{name}: {error}"));
    }
    Ok(value)
}

/// The access policy of the `use` profile: reading, plus exactly the kinds of data writes the
/// memory functions are inferred to make (their effects as the engine reports them). Fails
/// when a memory function is missing, cannot run, or would write anything but data.
pub fn use_policy(session: &Session) -> Result<AccessPolicy, String> {
    let mut policy = AccessPolicy::read_only();
    for name in FUNCTIONS {
        let entry = session.get_query(name).map_err(|err| err.to_string())?;
        if entry.is_null() {
            return Err(format!(
                "the memory function {name} is missing from the store"
            ));
        }
        if entry["status"] != "ok" {
            return Err(format!(
                "the memory function {name} cannot run: {}",
                entry["error"]
            ));
        }
        for effect in entry["effects"].as_array().into_iter().flatten() {
            let text = effect["text"].as_str().unwrap_or_default();
            let action = match effect["action"].as_str() {
                Some("Create") => ActionKind::Create,
                Some("Update") => ActionKind::Update,
                Some("Delete") => ActionKind::Delete,
                _ => return Err(format!("the memory function {name} has the effect {text}")),
            };
            if !text.contains("Data") {
                return Err(format!(
                    "the memory function {name} writes more than data: {text}"
                ));
            }
            policy = policy.allow(action, ResourceKind::Data);
        }
    }
    Ok(policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_function_has_a_spec_and_a_body() {
        let functions = functions().unwrap();
        let names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, FUNCTIONS);
        for function in &functions {
            let params: Vec<&str> = function.spec["params"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["name"].as_str().unwrap())
                .collect();
            assert_eq!(
                params.contains(&PROJECT_PARAM),
                PROJECT_BOUND.contains(&function.name.as_str()),
                "{}: the project parameter and PROJECT_BOUND disagree",
                function.name
            );
            let body = function.spec["body"].as_str().unwrap();
            assert!(
                body.starts_with(&format!("fn {}(", function.name)),
                "{}",
                function.name
            );
            let description = function.spec["description"].as_str().unwrap();
            assert!(
                description.len() < 400,
                "{}: keep descriptions short",
                function.name
            );
        }
    }

    #[test]
    fn internal_functions_have_bodies_and_are_no_tools() {
        let internal = internal_functions().unwrap();
        let names: Vec<&str> = internal.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, INTERNAL_FUNCTIONS);
        for name in INTERNAL_FUNCTIONS {
            assert!(!FUNCTIONS.contains(&name), "{name} must not be a tool");
            assert!(body(name).unwrap().starts_with(&format!("fn {name}(")));
        }
        let mut session = Session::new();
        apply(&mut session).unwrap();
        for name in INTERNAL_FUNCTIONS {
            let entry = session.get_query(name).unwrap();
            assert_eq!(entry["status"], "ok", "{name}: {entry}");
        }
        assert_eq!(
            call(&mut session, "indexed_sessions", &json!({})).unwrap(),
            json!([])
        );
        let err = call(&mut session, "no_such_query", &json!({})).unwrap_err();
        assert!(err.starts_with("no_such_query:"), "{err}");
    }

    #[test]
    fn the_schema_is_valid_json_with_the_memory_labels() {
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        for label in ["memory", "session", "turn", "fact"] {
            assert!(
                schema["vertices"][label]["schema"]["properties"]["project"].is_object(),
                "{label}"
            );
            assert_eq!(
                schema["vertices"][label]["schema"]["properties"]["emb"]["embedder"],
                "hash-ngram/1"
            );
        }
        for edge in ["relates_to", "supersedes", "about", "contains", "edited"] {
            assert!(schema["edges"][edge].is_object(), "{edge}");
        }
    }

    #[test]
    fn apply_writes_a_new_store_once_and_replaces_an_edited_product_query() {
        let mut session = Session::new();
        assert_eq!(apply(&mut session).unwrap(), Applied::Written);
        assert_eq!(apply(&mut session).unwrap(), Applied::Current);
        let schema = session.get_schema().unwrap();
        assert!(schema["vertices"]["memory"].is_object(), "{schema}");

        // A local edit drops the product mark: the next start writes the catalog again.
        let edited = session.define_query(&json!({
            "name": "session", "replaces": "session", "folder": FOLDER,
            "params": [{"name": "id", "schema": {"type": "string"}},
                       {"name": "turns", "schema": {"type": "boolean"}, "default": false}],
            "body": "fn session(id, turns) { 1 }",
        }));
        assert_eq!(edited["ok"], true, "{edited}");
        // A product query this binary no longer has is dropped; a user's own query stays.
        for (name, meta) in [("old_product", meta()), ("mine", String::new())] {
            let mut spec = json!({"name": name, "body": format!("fn {name}() {{ 1 }}")});
            if !meta.is_empty() {
                spec["meta"] = json!(meta);
            }
            assert_eq!(session.define_query(&spec)["ok"], true);
        }
        assert_eq!(apply(&mut session).unwrap(), Applied::Written);
        assert!(session.get_query("old_product").unwrap().is_null());
        assert!(!session.get_query("mine").unwrap().is_null());
        let restored = session.get_query("session").unwrap();
        assert!(restored["body"].as_str().unwrap().contains("turn_list"));
        assert_eq!(apply(&mut session).unwrap(), Applied::Current);
    }

    #[test]
    fn the_use_policy_allows_data_writes_only() {
        let mut session = Session::new();
        apply(&mut session).unwrap();
        let policy = use_policy(&session).unwrap();
        for action in [ActionKind::Create, ActionKind::Update, ActionKind::Delete] {
            assert!(policy.allows(action, ResourceKind::Data), "{action:?}");
            for resource in [ResourceKind::Schema, ResourceKind::Definition] {
                assert!(!policy.allows(action, resource), "{action:?} {resource:?}");
            }
        }
        assert!(policy.allows(ActionKind::Read, ResourceKind::Data));
    }

    #[test]
    fn the_fingerprint_is_stable_and_marks_product_queries() {
        assert_eq!(fingerprint(), fingerprint());
        assert_eq!(fingerprint().len(), 16);
        assert_eq!(product_fingerprint(&json!(meta())), Some(fingerprint()));
        assert_eq!(product_fingerprint(&json!("{\"origin\":\"user\"}")), None);
        assert_eq!(product_fingerprint(&Value::Null), None);
    }
}
