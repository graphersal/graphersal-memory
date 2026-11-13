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

/// Name, description and parameters of the browser UI's own functions (`graphersal-memory ui`):
/// reads and the reordering the UI needs beside the memory functions, never offered as tools of
/// the agent's servers (the daemon's relay context selects them, `crate::relay`).
const UI: &str = include_str!("catalog/ui.json");

/// The browser UI's functions, in the order of `catalog/ui.json`.
pub const UI_FUNCTIONS: [&str; 6] = [
    "overview",
    "memory_list",
    "set_order",
    "session_list",
    "memory_graph",
    "memory_neighbours",
];

/// The internal functions, in the order of `catalog/internal.json`.
pub const INTERNAL_FUNCTIONS: [&str; 12] = [
    "indexed_sessions",
    "index_session",
    "index_turns",
    "set_summary",
    "merge_project",
    "export_memories",
    "start_context",
    "open_conflicts",
    "migrate_import_sources",
    "link_memories",
    "sync_instructions",
    "scan_instructions",
];

/// The folder of the product's saved queries.
pub const FOLDER: &str = "memory";

/// The memory functions, in the order of the `use` profile's tool list.
pub const FUNCTIONS: [&str; 10] = [
    "memories",
    "memory",
    "remember",
    "forget",
    "pin",
    "conflicts",
    "settle",
    "sessions",
    "session",
    "recall",
];

/// The functions that declare the `project` parameter, which each MCP connection binds to its
/// project (`graphersal_mcp::Profile::with_bound_params` binds it for every saved query that
/// declares it; a test keeps this list and the declarations in step).
#[cfg(test)]
pub const PROJECT_BOUND: [&str; 10] = [
    "memories",
    "memory",
    "remember",
    "forget",
    "pin",
    "conflicts",
    "settle",
    "sessions",
    "session",
    "recall",
];

/// The name of the parameter bound to the connection's project (`crate::mcp`).
pub const PROJECT_PARAM: &str = "project";

/// The name of the parameter that says who writes (`remember`, `forget`): bound to the agent
/// on every MCP connection (`crate::mcp::AGENT_SOURCE`), `"ui"` from the browser UI.
pub const SOURCE_PARAM: &str = "source";

/// The body of the memory function `name`.
fn body(name: &str) -> Option<&'static str> {
    Some(match name {
        "memories" => include_str!("catalog/memories.rhai"),
        "memory" => include_str!("catalog/memory.rhai"),
        "remember" => include_str!("catalog/remember.rhai"),
        "forget" => include_str!("catalog/forget.rhai"),
        "pin" => include_str!("catalog/pin.rhai"),
        "conflicts" => include_str!("catalog/conflicts.rhai"),
        "settle" => include_str!("catalog/settle.rhai"),
        "sessions" => include_str!("catalog/sessions.rhai"),
        "session" => include_str!("catalog/session.rhai"),
        "recall" => include_str!("catalog/recall.rhai"),
        "overview" => include_str!("catalog/overview.rhai"),
        "memory_list" => include_str!("catalog/memory_list.rhai"),
        "set_order" => include_str!("catalog/set_order.rhai"),
        "session_list" => include_str!("catalog/session_list.rhai"),
        "memory_graph" => include_str!("catalog/memory_graph.rhai"),
        "memory_neighbours" => include_str!("catalog/memory_neighbours.rhai"),
        "indexed_sessions" => include_str!("catalog/indexed_sessions.rhai"),
        "index_session" => include_str!("catalog/index_session.rhai"),
        "index_turns" => include_str!("catalog/index_turns.rhai"),
        "set_summary" => include_str!("catalog/set_summary.rhai"),
        "merge_project" => include_str!("catalog/merge_project.rhai"),
        "export_memories" => include_str!("catalog/export_memories.rhai"),
        "start_context" => include_str!("catalog/start_context.rhai"),
        "open_conflicts" => include_str!("catalog/open_conflicts.rhai"),
        "migrate_import_sources" => include_str!("catalog/migrate_import_sources.rhai"),
        "link_memories" => include_str!("catalog/link_memories.rhai"),
        "sync_instructions" => include_str!("catalog/sync_instructions.rhai"),
        "scan_instructions" => include_str!("catalog/scan_instructions.rhai"),
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

/// The browser UI's functions of this binary's catalog.
pub fn ui_functions() -> Result<Vec<Function>, String> {
    parse_functions(UI)
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
    feed(UI);
    for name in FUNCTIONS
        .iter()
        .chain(INTERNAL_FUNCTIONS.iter())
        .chain(UI_FUNCTIONS.iter())
    {
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
    wanted.extend(ui_functions()?);
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
    // The store keeps the embedder it has (`StoreRuntime::switch_embedder` changes it).
    let embedder = current_embedder(session)?;
    session
        .set_schema(&schema_with(&embedder), None)
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

/// The embedder the memory schema of `session` names (the model identity the store recorded,
/// or `hash-ngram/1`); `hash-ngram/1` for a store without a schema.
pub fn current_embedder(session: &Session) -> Result<String, String> {
    let schema = session.get_schema().map_err(|err| err.to_string())?;
    Ok(
        schema["vertices"]["memory"]["schema"]["properties"]["emb"]["embedder"]
            .as_str()
            .unwrap_or(crate::models::BUILTIN)
            .to_string(),
    )
}

/// The memory schema with every vector rule's embedder set to `embedder` (a model repository
/// or identity the store holds, or `hash-ngram/1`).
pub fn schema_with(embedder: &str) -> String {
    SCHEMA.replace(
        "\"embedder\": \"hash-ngram/1\"",
        &format!("\"embedder\": {}", json!(embedder)),
    )
}

/// The memory schema without its vector rules (`emb` undeclared): the first step of an embedder
/// change whose vectors have another dimension (the engine checks the existing vectors against
/// the new declaration before the rule's fill replaces them, so the old vectors go first).
pub fn schema_without_vectors() -> Result<String, String> {
    let mut schema: Value = serde_json::from_str(SCHEMA)
        .map_err(|err| format!("the built-in schema is not valid JSON: {err}"))?;
    if let Some(labels) = schema["vertices"].as_object_mut() {
        for label in labels.values_mut() {
            if let Some(properties) = label["schema"]["properties"].as_object_mut() {
                properties.remove("emb");
            }
        }
    }
    Ok(schema.to_string())
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
    policy_of(session, &FUNCTIONS)
}

/// The access policy of the browser UI's relay context (`crate::relay`): reading, plus the
/// kinds of data writes the memory functions and the UI's functions are inferred to make.
pub fn ui_policy(session: &Session) -> Result<AccessPolicy, String> {
    let names: Vec<&str> = FUNCTIONS
        .iter()
        .chain(UI_FUNCTIONS.iter())
        .copied()
        .collect();
    policy_of(session, &names)
}

/// Reading, plus the data writes the functions `names` are inferred to make.
fn policy_of(session: &Session, names: &[&str]) -> Result<AccessPolicy, String> {
    let mut policy = AccessPolicy::read_only();
    for &name in names {
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
    fn the_schema_names_another_embedder_everywhere() {
        let schema: Value = serde_json::from_str(&schema_with("acme/model")).unwrap();
        for label in ["memory", "session", "turn", "fact"] {
            assert_eq!(
                schema["vertices"][label]["schema"]["properties"]["emb"]["embedder"],
                "acme/model"
            );
        }
        let mut session = Session::new();
        assert_eq!(current_embedder(&session).unwrap(), "hash-ngram/1");
        apply(&mut session).unwrap();
        assert_eq!(current_embedder(&session).unwrap(), "hash-ngram/1");
    }

    #[test]
    fn merge_project_moves_memories_edges_and_sessions() {
        let mut session = Session::new();
        apply(&mut session).unwrap();
        let run = |session: &mut Session, script: &str| {
            let answer: Value = serde_json::from_str(
                &session.execute(script, &graphersal_session::RunOptions::default()),
            )
            .unwrap();
            assert_eq!(answer["ok"], true, "{answer}");
            answer["execution"]["results"].clone()
        };
        for (slug, project, summary) in [
            ("a", "/old", "only in old"),
            ("b", "/old", "old b"),
            ("b", "/new", "new b, newer"),
            ("g", "", "global"),
        ] {
            run(
                &mut session,
                &format!(
                    "g.query(\"remember\", #{{slug: \"{slug}\", kind: \"project\", summary: \"{summary}\", body: \"x\", why: \"\", how_to_apply: \"\", title: \"\", hook: \"\", links: [], about: [], scope: \"{}\", project: \"{project}\"}})",
                    if project.is_empty() {
                        "global"
                    } else {
                        "project"
                    }
                ),
            );
        }
        run(
            &mut session,
            "g.query(\"remember\", #{slug: \"a\", kind: \"project\", summary: \"only in old\", body: \"x\", why: \"w\", how_to_apply: \"\", title: \"\", hook: \"\", links: [\"g\"], about: [], scope: \"project\", project: \"/old\"})",
        );
        run(
            &mut session,
            "g.add_v(\"session\").property(T.id, \"session:s\").property(\"name\", \"s\").property(\"started_at\", \"2026-11-10T00:00:00Z\").property(\"project\", \"/old\").iterate()",
        );
        let merged = call(
            &mut session,
            "merge_project",
            &json!({"from": "/old", "into": "/new"}),
        )
        .unwrap();
        assert_eq!(merged["memories_moved"], 1, "{merged}");
        assert_eq!(merged["memories_kept"], 1, "{merged}");
        assert_eq!(merged["others"], 1, "{merged}");
        let left = run(
            &mut session,
            "g.v().has(\"project\", \"/old\").count().next()",
        );
        assert_eq!(left, json!([0]));
        let moved = run(
            &mut session,
            "g.v(\"memory:a@/new\").project(\"why\", \"links\").by(\"why\").by(__.out(\"relates_to\").id().fold()).next()",
        );
        assert_eq!(moved[0]["why"], "w", "{moved}");
        assert_eq!(moved[0]["links"], json!(["memory:g"]), "{moved}");
        let kept = run(
            &mut session,
            "g.v(\"memory:b@/new\").values(\"summary\").next()",
        );
        assert_eq!(kept, json!(["new b, newer"]));
        let session_project = run(
            &mut session,
            "g.v(\"session:s\").values(\"project\").next()",
        );
        assert_eq!(session_project, json!(["/new"]));
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

    /// A session with the catalog and the memories `(slug, scope, summary)` of project `/p`.
    fn conflict_store(memories: &[(&str, &str, &str)]) -> Session {
        let mut session = Session::new();
        apply(&mut session).unwrap();
        for (slug, scope, summary) in memories {
            remember_in(
                &mut session,
                json!({"slug": slug, "summary": summary, "scope": scope}),
            );
        }
        session
    }

    /// `remember` in project `/p` with `args` over the defaults (kind project, body = summary).
    fn remember_in(session: &mut Session, args: Value) -> Value {
        let mut full = json!({"kind": "project", "body": args["summary"], "project": "/p"});
        for (key, value) in args.as_object().unwrap() {
            full[key] = value.clone();
        }
        call(session, "remember", &full).unwrap()
    }

    fn call_in(session: &mut Session, name: &str, args: Value) -> Value {
        let mut full = json!({"project": "/p"});
        for (key, value) in args.as_object().unwrap() {
            full[key] = value.clone();
        }
        call(session, name, &full).unwrap()
    }

    #[test]
    fn the_schema_declares_the_conflict_edges_and_import_marks() {
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        for edge in ["conflicts_with", "narrows"] {
            assert!(schema["edges"][edge].is_object(), "{edge}");
            assert!(schema["edges"][edge]["schema"]["properties"]["at"].is_object());
        }
        assert!(schema["edges"]["supersedes"]["schema"]["properties"]["at"].is_object());
        let memory = &schema["vertices"]["memory"]["schema"]["properties"];
        assert_eq!(memory["imported_at"]["format"], "date-time");
        assert_eq!(memory["unchecked"]["type"], "boolean");
    }

    #[test]
    fn remember_answers_the_nearest_memories_and_the_check_only_when_there_are_any() {
        let mut session = conflict_store(&[]);
        let first = remember_in(
            &mut session,
            json!({"slug": "use-npm", "summary": "Install dependencies with npm install"}),
        );
        assert!(first.get("nearest").is_none(), "{first}");
        assert!(first.get("check").is_none(), "{first}");
        remember_in(
            &mut session,
            json!({"slug": "never-push", "summary": "Never push without asking", "scope": "global"}),
        );
        let second = remember_in(
            &mut session,
            json!({"slug": "pnpm-instead-of-npm",
                   "summary": "Since today we install dependencies with pnpm instead of npm"}),
        );
        let nearest = second["nearest"].as_array().unwrap();
        assert!(nearest.len() <= 5, "{second}");
        assert_eq!(nearest[0]["slug"], "use-npm", "{second}");
        for field in ["slug", "scope", "kind", "summary", "source", "updated_at"] {
            assert!(nearest[0].get(field).is_some(), "{field}: {second}");
        }
        assert_eq!(nearest[0]["source"], "remember");
        assert!(
            nearest.iter().all(|m| m["slug"] != "pnpm-instead-of-npm"),
            "never itself: {second}"
        );
        assert!(
            nearest.iter().any(|m| m["scope"] == "global"),
            "the global memories are candidates too: {second}"
        );
        let check = second["check"].as_str().unwrap();
        for word in [
            "compatible",
            "replaces",
            "supersedes: [old slug]",
            "contradicts",
            "settle(\"pnpm-instead-of-npm\", \"conflict\"",
            "narrows a global one is compatible",
            "file source",
        ] {
            assert!(check.contains(word), "{word}: {check}");
        }
    }

    #[test]
    fn a_superseded_memory_is_marked_everywhere_and_never_starts_a_session() {
        let mut session = conflict_store(&[]);
        remember_in(
            &mut session,
            json!({"slug": "use-npm", "kind": "feedback", "summary": "Install dependencies with npm install"}),
        );
        let newer = remember_in(
            &mut session,
            json!({"slug": "pnpm-instead-of-npm", "kind": "feedback",
                   "summary": "We install dependencies with pnpm instead of npm",
                   "supersedes": ["use-npm", "no-such-memory"]}),
        );
        assert_eq!(newer["superseded"], json!(["use-npm"]), "{newer}");
        assert_eq!(
            newer["missing_targets"],
            json!(["no-such-memory"]),
            "{newer}"
        );
        assert!(
            newer["nearest"]
                .as_array()
                .is_none_or(|n| n.iter().all(|m| m["slug"] != "use-npm")),
            "a superseded memory is no candidate: {newer}"
        );

        let found = call_in(
            &mut session,
            "memories",
            json!({"text": "install dependencies npm", "k": 5, "kind": ""}),
        );
        let old = found
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["slug"] == "use-npm")
            .unwrap_or_else(|| panic!("marked, never hidden: {found}"));
        let summary = old["summary"].as_str().unwrap();
        assert!(
            summary.starts_with("SUPERSEDED by pnpm-instead-of-npm on 20"),
            "{summary}"
        );
        assert_eq!(old["superseded_by"], "pnpm-instead-of-npm");

        let listed = call_in(
            &mut session,
            "memories",
            json!({"text": "", "k": 10, "kind": ""}),
        );
        let old = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["slug"] == "use-npm")
            .unwrap();
        assert!(
            old["hook"]
                .as_str()
                .unwrap()
                .starts_with("SUPERSEDED by pnpm-instead-of-npm on "),
            "{old}"
        );

        let recalled = call_in(
            &mut session,
            "recall",
            json!({"text": "npm install", "k": 5, "label": "memory"}),
        );
        let old = recalled
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == "use-npm")
            .unwrap();
        assert!(
            old["summary"]
                .as_str()
                .unwrap()
                .starts_with("SUPERSEDED by ")
        );

        let read = call_in(&mut session, "memory", json!({"slug": "use-npm"}));
        assert_eq!(
            read["superseded_by"][0]["slug"], "pnpm-instead-of-npm",
            "{read}"
        );
        assert_eq!(
            read["chain"],
            json!(["use-npm", "pnpm-instead-of-npm"]),
            "{read}"
        );
        let read = call_in(
            &mut session,
            "memory",
            json!({"slug": "pnpm-instead-of-npm"}),
        );
        assert_eq!(read["supersedes"][0]["slug"], "use-npm", "{read}");
        assert_eq!(
            read["chain"],
            json!(["use-npm", "pnpm-instead-of-npm"]),
            "{read}"
        );
        assert_eq!(read["links"], json!([]), "supersedes is no link: {read}");

        let start = call_in(&mut session, "start_context", json!({}));
        let slugs: Vec<&str> = start
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["slug"].as_str().unwrap())
            .collect();
        assert_eq!(slugs, ["pnpm-instead-of-npm"], "{start}");
    }

    #[test]
    fn a_chain_of_three_decisions_reads_from_the_oldest_to_the_newest() {
        let mut session = conflict_store(&[
            ("node-18", "project", "The project targets Node 18"),
            ("node-20", "project", "The project targets Node 20"),
        ]);
        call_in(
            &mut session,
            "settle",
            json!({"slug": "node-20", "decision": "supersedes", "other": "node-18"}),
        );
        remember_in(
            &mut session,
            json!({"slug": "node-22", "summary": "Upgraded to Node 22", "supersedes": ["node-20"]}),
        );
        let read = call_in(&mut session, "memory", json!({"slug": "node-20"}));
        assert_eq!(
            read["chain"],
            json!(["node-18", "node-20", "node-22"]),
            "{read}"
        );
    }

    #[test]
    fn settle_opens_lists_and_closes_a_conflict_and_notes_a_narrowing() {
        let mut session = conflict_store(&[
            ("tabs", "project", "Indent with tabs"),
            ("spaces", "project", "Indent with four spaces"),
            ("small-commits", "global", "Keep commits small"),
            (
                "one-commit-per-pr",
                "project",
                "One commit per pull request in this project",
            ),
        ]);
        let opened = call_in(
            &mut session,
            "settle",
            json!({"slug": "spaces", "decision": "conflict", "other": "tabs"}),
        );
        assert_eq!(opened["open_conflicts"], 1, "{opened}");
        let refused = call(
            &mut session,
            "settle",
            &json!({"slug": "one-commit-per-pr", "decision": "conflict",
                    "other": "small-commits", "project": "/p"}),
        )
        .unwrap_err();
        assert!(refused.contains("narrows"), "{refused}");
        let noted = call_in(
            &mut session,
            "settle",
            json!({"slug": "one-commit-per-pr", "decision": "narrows", "other": "small-commits"}),
        );
        assert_eq!(
            noted["note"],
            "one-commit-per-pr narrows the global rule small-commits"
        );

        let listed = call_in(&mut session, "conflicts", json!({"k": 5}));
        let open = listed["open"].as_array().unwrap();
        assert_eq!(open.len(), 1, "{listed}");
        assert_eq!(open[0]["a"]["slug"], "spaces");
        assert_eq!(open[0]["b"]["slug"], "tabs");
        assert_eq!(open[0]["b"]["summary"], "Indent with tabs");
        assert!(
            listed["settle_open"]
                .as_str()
                .unwrap()
                .contains("supersedes")
        );
        assert_eq!(
            listed["narrows"][0]["note"], "narrows the global rule small-commits",
            "{listed}"
        );
        let read = call_in(&mut session, "memory", json!({"slug": "tabs"}));
        assert_eq!(read["conflicts_with"][0]["slug"], "spaces", "{read}");
        let found = call_in(
            &mut session,
            "memories",
            json!({"text": "indent with tabs", "k": 3, "kind": ""}),
        );
        let tabs = found
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["slug"] == "tabs")
            .unwrap();
        assert_eq!(tabs["open_conflict_with"][0]["slug"], "spaces", "{found}");
        let internal = call_in(&mut session, "open_conflicts", json!({}));
        assert_eq!(internal["open"][0]["a"]["slug"], "spaces", "{internal}");
        assert_eq!(internal["unchecked"], 0);

        // The user decided: spaces holds.
        let closed = call_in(
            &mut session,
            "settle",
            json!({"slug": "spaces", "decision": "supersedes", "other": "tabs"}),
        );
        assert_eq!(closed["open_conflicts"], 0, "{closed}");
        let listed = call_in(&mut session, "conflicts", json!({"k": 5}));
        assert_eq!(listed["open"], json!([]), "{listed}");
        let err = call(
            &mut session,
            "settle",
            &json!({"slug": "spaces", "decision": "supersedes", "project": "/p"}),
        )
        .unwrap_err();
        assert!(err.contains("needs the other memory"), "{err}");
    }

    #[test]
    fn an_imported_memory_keeps_its_file_and_decision_time_and_waits_for_its_check() {
        let mut session =
            conflict_store(&[("use-npm", "project", "Install dependencies with npm")]);
        let answer = remember_in(
            &mut session,
            json!({"slug": "use-yarn", "summary": "Install dependencies with yarn",
                   "from_file": {"path": "/home/u/.claude/projects/p/memory/use-yarn.md",
                                 "modified_at": "2026-10-01T08:00:00Z"}}),
        );
        assert_eq!(answer["unchecked"], true, "{answer}");
        let read = call_in(&mut session, "memory", json!({"slug": "use-yarn"}));
        assert_eq!(
            read["source"],
            "/home/u/.claude/projects/p/memory/use-yarn.md"
        );
        assert!(
            read["updated_at"].to_string().contains("2026-10-01"),
            "{read}"
        );
        assert!(read["imported_at"].is_string(), "{read}");
        assert_eq!(read["unchecked"], true);

        let listed = call_in(&mut session, "conflicts", json!({"k": 5}));
        assert_eq!(listed["unchecked_total"], 1, "{listed}");
        assert_eq!(listed["unchecked"][0]["slug"], "use-yarn");
        assert_eq!(listed["unchecked"][0]["nearest"][0]["slug"], "use-npm");
        assert!(listed["check"].is_string());

        // The agent judges it a conflict: the file is named so the user can edit it.
        let settled = call_in(
            &mut session,
            "settle",
            json!({"slug": "use-yarn", "decision": "conflict", "other": "use-npm"}),
        );
        assert_eq!(settled["unchecked"], 0, "{settled}");
        assert_eq!(
            settled["edit_files"][0]["file"],
            "/home/u/.claude/projects/p/memory/use-yarn.md"
        );
        let listed = call_in(&mut session, "conflicts", json!({"k": 5}));
        assert_eq!(
            listed["open"][0]["files"],
            json!(["/home/u/.claude/projects/p/memory/use-yarn.md"]),
            "{listed}"
        );
        let superseded = remember_in(
            &mut session,
            json!({"slug": "use-pnpm", "summary": "We use pnpm now", "supersedes": ["use-yarn"]}),
        );
        assert_eq!(
            superseded["edit_files"][0]["file"], "/home/u/.claude/projects/p/memory/use-yarn.md",
            "{superseded}"
        );
        let forgotten = call_in(&mut session, "forget", json!({"slug": "use-yarn"}));
        assert_eq!(
            forgotten["file"],
            "/home/u/.claude/projects/p/memory/use-yarn.md"
        );
        // The agent rewriting an imported memory makes it its own.
        remember_in(
            &mut session,
            json!({"slug": "use-npm2", "summary": "x", "from_file": {"path": "/f.md"}}),
        );
        remember_in(&mut session, json!({"slug": "use-npm2", "summary": "x"}));
        let read = call_in(&mut session, "memory", json!({"slug": "use-npm2"}));
        assert_eq!(read["source"], "remember", "{read}");
        assert!(read.get("imported_at").is_none(), "{read}");
    }

    #[test]
    fn merge_project_keeps_the_conflict_edges_with_their_time() {
        let mut session = Session::new();
        apply(&mut session).unwrap();
        for slug in ["a", "b"] {
            call(
                &mut session,
                "remember",
                &json!({"slug": slug, "kind": "project", "summary": slug, "body": slug,
                        "project": "/old"}),
            )
            .unwrap();
        }
        call(
            &mut session,
            "settle",
            &json!({"slug": "b", "decision": "supersedes", "other": "a", "project": "/old"}),
        )
        .unwrap();
        call(
            &mut session,
            "merge_project",
            &json!({"from": "/old", "into": "/new"}),
        )
        .unwrap();
        let read = call(
            &mut session,
            "memory",
            &json!({"slug": "a", "project": "/new"}),
        )
        .unwrap();
        assert_eq!(read["superseded_by"][0]["slug"], "b", "{read}");
        assert!(
            read["superseded_by"][0]["at"]
                .as_str()
                .unwrap()
                .starts_with("20")
        );
    }

    #[test]
    fn ui_functions_have_bodies_and_are_neither_tools_nor_internal() {
        let ui = ui_functions().unwrap();
        let names: Vec<&str> = ui.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, UI_FUNCTIONS);
        for function in &ui {
            let name = function.name.as_str();
            assert!(!FUNCTIONS.contains(&name) && !INTERNAL_FUNCTIONS.contains(&name));
            assert!(body(name).unwrap().starts_with(&format!("fn {name}(")));
            let params: Vec<&str> = function.spec["params"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["name"].as_str().unwrap())
                .collect();
            // The UI passes the project itself (decision 3.5B): every one declares it.
            assert!(params.contains(&PROJECT_PARAM), "{name}");
        }
        let mut session = Session::new();
        apply(&mut session).unwrap();
        for name in UI_FUNCTIONS {
            let entry = session.get_query(name).unwrap();
            assert_eq!(entry["status"], "ok", "{name}: {entry}");
        }
        let policy = ui_policy(&session).unwrap();
        assert!(policy.allows(ActionKind::Update, ResourceKind::Data));
        assert!(!policy.allows(ActionKind::Update, ResourceKind::Schema));
    }

    #[test]
    fn memory_list_filters_pages_and_marks_sources() {
        let mut session = conflict_store(&[("old", "project", "We use npm")]);
        remember_in(
            &mut session,
            json!({"slug": "new", "summary": "We use pnpm now", "supersedes": ["old"], "kind": "feedback"}),
        );
        remember_in(
            &mut session,
            json!({"slug": "from-ui", "summary": "Written in the UI", "source": "ui", "scope": "global"}),
        );
        remember_in(
            &mut session,
            json!({"slug": "from-file", "summary": "Imported", "kind": "reference",
                   "from_file": {"path": "/h/.claude/projects/p/memory/from-file.md"}}),
        );
        call_in(
            &mut session,
            "pin",
            json!({"slug": "from-ui", "pinned": true, "order": 3}),
        );
        let list = |session: &mut Session, args: Value| {
            let mut full = json!({"kind": "", "scope": "", "source_kind": "", "superseded": false,
                                  "offset": 0, "limit": 100});
            for (key, value) in args.as_object().unwrap() {
                full[key] = value.clone();
            }
            call_in(session, "memory_list", full)
        };
        let all = list(&mut session, json!({}));
        let slugs: Vec<&str> = all["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["slug"].as_str().unwrap())
            .collect();
        // Start-context order (pinned, feedback, the rest), the replaced one left out.
        assert_eq!(slugs, ["from-ui", "new", "from-file"], "{all}");
        assert_eq!(all["total"], 3);
        let first = &all["rows"][0];
        assert_eq!(first["source"], "ui");
        assert_eq!(first["source_kind"], "ui");
        assert_eq!(first["scope"], "global");
        assert_eq!(first["project"], "");
        assert_eq!(first["pinned"], true);
        assert_eq!(first["order"], 3);
        assert_eq!(first["always"], true);
        assert_eq!(all["rows"][1]["source_kind"], "agent");
        assert_eq!(all["rows"][1]["project"], "/p");
        assert_eq!(all["rows"][2]["source_kind"], "file");
        assert!(all["rows"][2]["imported_at"].is_string(), "{all}");
        let with_old = list(&mut session, json!({"superseded": true}));
        let old = with_old["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["slug"] == "old")
            .unwrap();
        assert_eq!(old["superseded_by"], "new");
        assert_eq!(
            old["summary"], "We use npm",
            "plain, the UI marks it: {old}"
        );
        assert_eq!(
            list(&mut session, json!({"source_kind": "file"}))["total"],
            1
        );
        assert_eq!(list(&mut session, json!({"scope": "global"}))["total"], 1);
        assert_eq!(list(&mut session, json!({"kind": "feedback"}))["total"], 1);
        let page = list(&mut session, json!({"offset": 1, "limit": 1}));
        assert_eq!(page["total"], 3);
        assert_eq!(page["rows"].as_array().unwrap().len(), 1);
        assert_eq!(page["rows"][0]["slug"], "new");
        let read = call_in(&mut session, "memory", json!({"slug": "from-ui"}));
        assert_eq!(read["source"], "ui", "{read}");
        assert_eq!(read["pinned"], true, "{read}");
        assert_eq!(read["order"], 3, "{read}");
        let read = call_in(&mut session, "memory", json!({"slug": "new"}));
        assert_eq!(read["pinned"], false, "{read}");
        assert!(read.get("order").is_none(), "{read}");
    }

    #[test]
    fn the_ui_cannot_edit_or_forget_an_imported_memory_but_may_pin_it() {
        let mut session = conflict_store(&[]);
        remember_in(
            &mut session,
            json!({"slug": "imported", "summary": "From a file", "from_file": {"path": "/f/imported.md"}}),
        );
        let mut args = json!({"slug": "imported", "kind": "project", "summary": "edited",
                              "body": "edited", "source": "ui", "project": "/p"});
        let err = call(&mut session, "remember", &args).unwrap_err();
        assert!(
            err.contains("/f/imported.md") && err.contains("read-only"),
            "{err}"
        );
        let err = call(
            &mut session,
            "forget",
            &json!({"slug": "imported", "source": "ui", "project": "/p"}),
        )
        .unwrap_err();
        assert!(err.contains("edit or delete that file"), "{err}");
        let read = call_in(&mut session, "memory", json!({"slug": "imported"}));
        assert_eq!(read["summary"], "From a file", "unchanged: {read}");
        call_in(
            &mut session,
            "pin",
            json!({"slug": "imported", "pinned": true}),
        );
        // The agent (source remember) may still rewrite and forget it, as before.
        args["source"] = json!("remember");
        call(&mut session, "remember", &args).unwrap();
        assert_eq!(
            call_in(&mut session, "forget", json!({"slug": "imported"}))["removed"],
            true
        );
    }

    #[test]
    fn a_ui_memory_counts_as_written_in_the_graph_not_as_a_file() {
        let mut session = conflict_store(&[("tabs", "project", "Indent with tabs")]);
        remember_in(
            &mut session,
            json!({"slug": "spaces", "summary": "Indent with four spaces", "source": "ui"}),
        );
        call_in(
            &mut session,
            "settle",
            json!({"slug": "spaces", "decision": "conflict", "other": "tabs"}),
        );
        let listed = call_in(&mut session, "conflicts", json!({"k": 5}));
        let open = &listed["open"][0];
        assert_eq!(open["a"]["source"], "ui", "{listed}");
        assert!(open.get("files").is_none(), "no file to edit: {listed}");
        // An imported memory near a UI memory waits for its check (another source).
        let imported = remember_in(
            &mut session,
            json!({"slug": "indent-tabs", "summary": "Indent with tabs always",
                   "from_file": {"path": "/d/indent-tabs.md", "dir": "/d"}}),
        );
        assert_eq!(imported["unchecked"], true, "{imported}");
    }

    #[test]
    fn set_order_renumbers_in_one_unit_and_refuses_unknown_slugs() {
        let mut session = conflict_store(&[
            ("a", "project", "A"),
            ("b", "project", "B"),
            ("g", "global", "G"),
        ]);
        let done = call_in(&mut session, "set_order", json!({"slugs": ["b", "g", "a"]}));
        assert_eq!(done["reordered"], 3, "{done}");
        for (slug, order) in [("b", 10), ("g", 20), ("a", 30)] {
            assert_eq!(
                call_in(&mut session, "memory", json!({"slug": slug}))["order"],
                order
            );
        }
        let err = call(
            &mut session,
            "set_order",
            &json!({"slugs": ["a", "missing", "b"], "project": "/p"}),
        )
        .unwrap_err();
        assert!(
            err.contains("missing") && err.contains("nothing was reordered"),
            "{err}"
        );
        assert_eq!(
            call_in(&mut session, "memory", json!({"slug": "a"}))["order"],
            30
        );
        let err = call(
            &mut session,
            "set_order",
            &json!({"slugs": ["a", "a"], "project": "/p"}),
        )
        .unwrap_err();
        assert!(err.contains("twice"), "{err}");
        // Another project's memory is not this project's.
        let err = call(
            &mut session,
            "set_order",
            &json!({"slugs": ["a"], "project": "/other"}),
        )
        .unwrap_err();
        assert!(err.contains("no memory a"), "{err}");
    }

    #[test]
    fn overview_and_session_list_count_what_the_ui_shows() {
        let mut session = conflict_store(&[
            ("tabs", "project", "Indent with tabs"),
            ("spaces", "project", "Indent with spaces"),
            ("g", "global", "Global rule"),
        ]);
        remember_in(
            &mut session,
            json!({"slug": "me", "kind": "user", "summary": "I am me"}),
        );
        call_in(&mut session, "pin", json!({"slug": "tabs"}));
        call_in(
            &mut session,
            "settle",
            json!({"slug": "spaces", "decision": "conflict", "other": "tabs"}),
        );
        let run = |session: &mut Session, script: &str| {
            let answer: Value = serde_json::from_str(
                &session.execute(script, &graphersal_session::RunOptions::default()),
            )
            .unwrap();
            assert_eq!(answer["ok"], true, "{answer}");
        };
        for (id, at) in [
            ("s1", "2026-11-10T10:00:00Z"),
            ("s2", "2026-11-12T09:00:00Z"),
        ] {
            run(
                &mut session,
                &format!(
                    "g.add_v(\"session\").property(T.id, \"session:{id}\").property(\"name\", \"headline {id}\").property(\"started_at\", \"{at}\").property(\"harness\", \"claude-code\").property(\"project\", \"/p\").iterate()"
                ),
            );
        }
        run(
            &mut session,
            "g.add_v(\"fact\").property(T.id, \"fact:s2:decision:0\").property(\"name\", \"d\").property(\"summary\", \"Decided x\").property(\"fact_kind\", \"decision\").property(\"project\", \"/p\").as(\"f\").V(\"session:s2\").add_e(\"contains\").to(\"f\").iterate()",
        );
        let counts = call_in(&mut session, "overview", json!({}));
        assert_eq!(counts["memories"], 4, "{counts}");
        assert_eq!(counts["project_memories"], 3, "{counts}");
        assert_eq!(counts["global_memories"], 1, "{counts}");
        assert_eq!(counts["user"], 1);
        assert_eq!(counts["pinned"], 1);
        assert_eq!(counts["open_conflicts"], 1);
        assert_eq!(
            counts["always_loaded"], 2,
            "pinned tabs and the user memory: {counts}"
        );
        assert_eq!(counts["sessions"], 2);
        assert_eq!(counts["facts"], 1);
        let global = call(&mut session, "overview", &json!({"project": ""})).unwrap();
        assert_eq!(global["memories"], 1, "{global}");
        assert_eq!(global["sessions"], 0, "{global}");

        let list = call_in(
            &mut session,
            "session_list",
            json!({"before": "", "limit": 10}),
        );
        let rows = list.as_array().unwrap();
        assert_eq!(rows.len(), 2, "{list}");
        assert_eq!(rows[0]["id"], "session:s2");
        assert_eq!(rows[0]["decisions"], 1);
        assert_eq!(rows[0]["harness"], "claude-code");
        assert_eq!(rows[1]["decisions"], 0);
        let older = call_in(
            &mut session,
            "session_list",
            json!({"before": rows[0]["started_at"], "limit": 10}),
        );
        assert_eq!(older.as_array().unwrap().len(), 1, "{older}");
        assert_eq!(older[0]["id"], "session:s1");
        let other = call(
            &mut session,
            "session_list",
            &json!({"before": "", "limit": 10, "project": "/q"}),
        )
        .unwrap();
        assert_eq!(other, json!([]));
    }

    #[test]
    fn memory_graph_has_the_memories_and_their_decision_edges_and_neighbours_add_sessions() {
        let mut session = conflict_store(&[
            ("tabs", "project", "Indent with tabs"),
            ("spaces", "project", "Indent with spaces"),
            ("old-style", "project", "The old style"),
            ("g", "global", "Global rule"),
        ]);
        call_in(
            &mut session,
            "settle",
            json!({"slug": "spaces", "decision": "conflict", "other": "tabs"}),
        );
        remember_in(
            &mut session,
            json!({"slug": "new-style", "summary": "The new style", "supersedes": ["old-style"],
                   "links": ["tabs"]}),
        );
        call_in(
            &mut session,
            "settle",
            json!({"slug": "tabs", "decision": "narrows", "other": "g"}),
        );
        remember_in(
            &mut session,
            json!({"slug": "imported", "summary": "From a file", "kind": "feedback",
                   "from_file": {"path": "/f/imported.md"}}),
        );
        // Another project's memory is not in this project's graph.
        call(
            &mut session,
            "remember",
            &json!({"slug": "elsewhere", "kind": "project", "summary": "x", "body": "x",
                    "project": "/q"}),
        )
        .unwrap();
        let graph = call_in(&mut session, "memory_graph", json!({}));
        let nodes = graph["nodes"].as_array().unwrap();
        let slugs: Vec<&str> = nodes.iter().map(|n| n["slug"].as_str().unwrap()).collect();
        for slug in ["tabs", "spaces", "old-style", "new-style", "g", "imported"] {
            assert!(slugs.contains(&slug), "{slug}: {graph}");
        }
        assert!(!slugs.contains(&"elsewhere"), "{graph}");
        let node = |slug: &str| nodes.iter().find(|n| n["slug"] == slug).unwrap().clone();
        assert_eq!(node("old-style")["superseded_by"], "new-style", "{graph}");
        assert_eq!(node("old-style")["always"], false);
        assert_eq!(node("g")["scope"], "global");
        assert_eq!(node("g")["project"], "");
        assert_eq!(node("tabs")["id"], "memory:tabs@/p");
        assert_eq!(node("tabs")["conflicts"], json!(["spaces"]));
        assert_eq!(node("imported")["source_kind"], "file");
        assert_eq!(node("imported")["always"], true, "a feedback memory");
        assert!(node("imported")["imported_at"].is_string());
        let edges = graph["edges"].as_array().unwrap();
        let has = |label: &str, from: &str, to: &str| {
            edges
                .iter()
                .any(|e| e["label"] == label && e["source"] == from && e["target"] == to)
        };
        assert!(
            has("supersedes", "memory:new-style@/p", "memory:old-style@/p"),
            "{graph}"
        );
        assert!(
            has("relates_to", "memory:new-style@/p", "memory:tabs@/p"),
            "{graph}"
        );
        assert!(has("narrows", "memory:tabs@/p", "memory:g"), "{graph}");
        assert!(
            edges.iter().any(|e| e["label"] == "conflicts_with"
                && [e["source"].clone(), e["target"].clone()].contains(&json!("memory:tabs@/p"))),
            "{graph}"
        );
        assert!(
            edges
                .iter()
                .all(|e| e["id"].is_string() || e["id"].is_number())
        );
        assert!(
            edges
                .iter()
                .any(|e| e["label"] == "supersedes" && e["at"].as_str().unwrap().starts_with("20"))
        );
        // The global graph: the global memory only.
        let global = call(&mut session, "memory_graph", &json!({"project": ""})).unwrap();
        assert_eq!(global["nodes"].as_array().unwrap().len(), 1, "{global}");
        assert_eq!(global["edges"], json!([]));

        // A session with a fact; the memory tabs is about the fact.
        let run = |session: &mut Session, script: &str| {
            let answer: Value = serde_json::from_str(
                &session.execute(script, &graphersal_session::RunOptions::default()),
            )
            .unwrap();
            assert_eq!(answer["ok"], true, "{answer}");
        };
        run(
            &mut session,
            "g.add_v(\"session\").property(T.id, \"session:s1\").property(\"name\", \"headline s1\").property(\"started_at\", \"2026-11-12T09:00:00Z\").property(\"harness\", \"claude-code\").property(\"project\", \"/p\").iterate()",
        );
        run(
            &mut session,
            "g.add_v(\"fact\").property(T.id, \"fact:s1:decision:0\").property(\"name\", \"d\").property(\"summary\", \"Decided tabs\").property(\"why\", \"style\").property(\"fact_kind\", \"decision\").property(\"project\", \"/p\").as(\"f\").V(\"session:s1\").add_e(\"contains\").to(\"f\").iterate()",
        );
        run(
            &mut session,
            "g.add_v(\"session\").property(T.id, \"session:q1\").property(\"name\", \"other project\").property(\"started_at\", \"2026-11-12T09:00:00Z\").property(\"project\", \"/q\").iterate()",
        );
        remember_in(
            &mut session,
            json!({"slug": "tabs", "summary": "Indent with tabs",
                   "about": ["fact:s1:decision:0", "memory:g"]}),
        );
        let around = call_in(&mut session, "memory_neighbours", json!({"slug": "tabs"}));
        assert_eq!(around["memory"], "memory:tabs@/p", "{around}");
        let ids: Vec<&str> = around["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids.len(),
            2,
            "the fact and its session, no memory: {around}"
        );
        assert!(
            ids.contains(&"fact:s1:decision:0") && ids.contains(&"session:s1"),
            "{around}"
        );
        let fact = around["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["label"] == "fact")
            .unwrap();
        assert_eq!(fact["fact_kind"], "decision");
        assert_eq!(fact["why"], "style");
        assert_eq!(fact["session"], "session:s1");
        assert_eq!(fact["headline"], "headline s1");
        assert_eq!(fact["project"], "/p");
        let edges = around["edges"].as_array().unwrap();
        let has = |label: &str, from: &str, to: &str| {
            edges
                .iter()
                .any(|e| e["label"] == label && e["source"] == from && e["target"] == to)
        };
        assert!(
            has("about", "memory:tabs@/p", "fact:s1:decision:0"),
            "{around}"
        );
        assert!(
            has("contains", "session:s1", "fact:s1:decision:0"),
            "{around}"
        );
        assert!(
            has("about", "memory:tabs@/p", "memory:g"),
            "an edge may end at a memory: {around}"
        );
        assert!(
            around["memories"]
                .as_array()
                .unwrap()
                .contains(&json!("memory:g"))
        );
        // Nothing around a memory without about edges; an unknown memory is an error answer.
        let lonely = call_in(&mut session, "memory_neighbours", json!({"slug": "spaces"}));
        assert_eq!(lonely["nodes"], json!([]), "{lonely}");
        assert_eq!(lonely["edges"], json!([]), "{lonely}");
        let unknown = call(
            &mut session,
            "memory_neighbours",
            &json!({"slug": "nope", "project": "/p"}),
        )
        .unwrap_err();
        assert!(unknown.contains("no memory nope"), "{unknown}");
        // Another project sees neither the session nor the fact.
        call(
            &mut session,
            "remember",
            &json!({"slug": "g", "kind": "project", "summary": "Global rule", "body": "x",
                    "about": ["session:q1"], "scope": "global", "project": "/q"}),
        )
        .unwrap();
        let from_p = call_in(&mut session, "memory_neighbours", json!({"slug": "g"}));
        assert!(
            !from_p["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["id"] == "session:q1"),
            "{from_p}"
        );
    }

    #[test]
    fn expanding_an_imported_memory_names_its_file_and_link_memories_is_idempotent() {
        let mut session = Session::new();
        apply(&mut session).unwrap();
        remember_in(
            &mut session,
            json!({"slug": "tabs", "summary": "Indent with tabs"}),
        );
        let answer: Value = serde_json::from_str(&session.execute(
            "g.v(\"memory:tabs@/p\").property(\"imported_at\", \"2026-11-01T00:00:00Z\")\
             .property(\"source\", \"/h/memory/tabs.md\").iterate()",
            &graphersal_session::RunOptions::default(),
        ))
        .unwrap();
        assert_eq!(answer["ok"], true, "{answer}");
        let around = call_in(&mut session, "memory_neighbours", json!({"slug": "tabs"}));
        assert_eq!(around["imported"], "/h/memory/tabs.md", "{around}");
        assert_eq!(around["nodes"], json!([]));
        // link_memories: an unknown session is an error answer; a repeated call the same edges.
        let unknown = call(
            &mut session,
            "link_memories",
            &json!({"id": "session:nope", "links": []}),
        )
        .unwrap_err();
        assert!(unknown.contains("no session"), "{unknown}");
        let answer: Value = serde_json::from_str(&session.execute(
            "g.add_v(\"session\").property(T.id, \"session:s9\").property(\"name\", \"s9\").property(\"started_at\", \"2026-11-12T09:00:00Z\").property(\"project\", \"/p\").iterate()",
            &graphersal_session::RunOptions::default(),
        ))
        .unwrap();
        assert_eq!(answer["ok"], true, "{answer}");
        let links = json!([{"slug": "tabs", "actions": ["remember"], "at": "", "last_at": "", "count": 1},
                           {"slug": "gone", "actions": ["forget"], "at": "", "last_at": "", "count": 1}]);
        for _ in 0..2 {
            let done = call(
                &mut session,
                "link_memories",
                &json!({"id": "session:s9", "links": links}),
            )
            .unwrap();
            assert_eq!(done["linked"], json!(["memory:tabs@/p"]), "{done}");
            assert_eq!(done["missing"], json!(["gone"]), "{done}");
        }
        let around = call_in(&mut session, "memory_neighbours", json!({"slug": "tabs"}));
        let wrote: Vec<&Value> = around["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["label"] == "wrote")
            .collect();
        assert_eq!(wrote.len(), 1, "{around}");
    }

    #[test]
    fn the_fingerprint_is_stable_and_marks_product_queries() {
        assert_eq!(fingerprint(), fingerprint());
        assert_eq!(fingerprint().len(), 16);
        assert_eq!(product_fingerprint(&json!(meta())), Some(fingerprint()));
        assert_eq!(product_fingerprint(&json!("{\"origin\":\"user\"}")), None);
        assert_eq!(product_fingerprint(&Value::Null), None);
    }

    /// A rule of an instruction file as `crate::instructions` sends it to `sync_instructions`.
    fn rule(id: &str, text: &str, file: &str, line: u64, global: bool) -> Value {
        json!({"id": format!("instruction:{id}"), "text": text, "file": file, "line": line,
               "loaded_by": "claude-code", "global": global})
    }

    /// `sync_instructions` then `scan_instructions` until nothing is left, in project `/p`.
    fn sync_rules(session: &mut Session, rules: Vec<Value>) -> Value {
        let synced = call_in(session, "sync_instructions", json!({"rules": rules}));
        loop {
            let scan = call_in(session, "scan_instructions", json!({"limit": 3}));
            if scan["remaining"] == 0 || scan["capped"] == true {
                break;
            }
        }
        synced
    }

    const NPM: &str = "Always use npm for installing packages, never another package manager.";

    #[test]
    fn the_schema_declares_instruction_lines_and_their_comparisons() {
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let instruction = &schema["vertices"]["instruction"]["schema"];
        assert_eq!(instruction["required"], json!(["text", "file", "line"]));
        assert_eq!(instruction["properties"]["emb"]["from"], "embed_text");
        let edge = &schema["edges"]["compared_with"];
        assert_eq!(
            edge["connections"],
            json!([{"from": "instruction", "to": "memory"}])
        );
        assert_eq!(
            edge["schema"]["properties"]["decision"]["enum"],
            json!(["unchecked", "compatible", "conflict"])
        );
    }

    #[test]
    fn a_rule_near_a_memory_becomes_a_candidate_and_settles_compatible_or_conflict() {
        let mut session = conflict_store(&[
            (
                "use-pnpm",
                "project",
                "Install dependencies with pnpm, not npm",
            ),
            (
                "dark-theme",
                "global",
                "The user prefers a dark editor theme",
            ),
        ]);
        let synced = sync_rules(
            &mut session,
            vec![
                rule("00000000000000a1", NPM, "/p/CLAUDE.md", 3, false),
                rule(
                    "00000000000000a2",
                    "Write commit messages in the imperative mood.",
                    "/p/CLAUDE.md",
                    5,
                    false,
                ),
            ],
        );
        assert_eq!(synced["created"], 2, "{synced}");
        let listed = call_in(&mut session, "conflicts", json!({"k": 5}));
        let pairs = listed["rules_unchecked"].as_array().expect("candidates");
        let npm = pairs
            .iter()
            .find(|p| p["memory"]["slug"] == "use-pnpm")
            .unwrap_or_else(|| panic!("{listed}"));
        assert_eq!(npm["rule"]["rule"], "00000000000000a1", "{listed}");
        assert_eq!(npm["rule"]["file"], "/p/CLAUDE.md");
        assert_eq!(npm["rule"]["line"], 3);
        assert_eq!(npm["rule"]["text"], NPM);
        assert_eq!(npm["rule"]["scope"], "project");
        assert!(
            listed["check_rules"]
                .as_str()
                .unwrap()
                .contains("rule: <rule>")
        );
        let total = listed["rules_unchecked_total"].as_u64().unwrap();
        let internal = call_in(&mut session, "open_conflicts", json!({}));
        assert_eq!(internal["rules_unchecked"], total, "{internal}");
        assert_eq!(internal["rules_open"], json!([]));

        // Contradiction: open everywhere until resolved.
        let settled = call_in(
            &mut session,
            "settle",
            json!({"slug": "use-pnpm", "decision": "conflict", "rule": "00000000000000a1"}),
        );
        assert_eq!(settled["rule_conflicts"], 1, "{settled}");
        assert!(
            settled["note"]
                .as_str()
                .unwrap()
                .contains("/p/CLAUDE.md line 3"),
            "{settled}"
        );
        let listed = call_in(&mut session, "conflicts", json!({"k": 5}));
        assert_eq!(
            listed["rules_open"][0]["memory"]["slug"], "use-pnpm",
            "{listed}"
        );
        assert!(
            listed["settle_rules"]
                .as_str()
                .unwrap()
                .contains("never edit it yourself")
        );
        let internal = call_in(&mut session, "open_conflicts", json!({}));
        assert_eq!(
            internal["rules_open"][0]["memory"]["slug"], "use-pnpm",
            "{internal}"
        );
        assert_eq!(internal["rules_open"][0]["line"], 3);
        let overview = call_in(&mut session, "overview", json!({}));
        assert_eq!(overview["rule_conflicts"], 1, "{overview}");

        // Both hold after all: dismissed, not listed again.
        let settled = call_in(
            &mut session,
            "settle",
            json!({"slug": "use-pnpm", "decision": "compatible", "rule": "00000000000000a1"}),
        );
        assert_eq!(settled["rule_conflicts"], 0, "{settled}");
        let listed = call_in(&mut session, "conflicts", json!({"k": 5}));
        assert!(listed.get("rules_open").is_none(), "{listed}");
        assert!(
            listed["rules_unchecked"]
                .as_array()
                .into_iter()
                .flatten()
                .all(|p| p["rule"]["rule"] != "00000000000000a1"
                    || p["memory"]["slug"] != "use-pnpm"),
            "{listed}"
        );
        // A scan again (nothing changed) brings nothing back.
        sync_rules(
            &mut session,
            vec![
                rule("00000000000000a1", NPM, "/p/CLAUDE.md", 3, false),
                rule(
                    "00000000000000a2",
                    "Write commit messages in the imperative mood.",
                    "/p/CLAUDE.md",
                    5,
                    false,
                ),
            ],
        );
        let again = call_in(&mut session, "conflicts", json!({"k": 5}));
        assert!(
            again["rules_unchecked"]
                .as_array()
                .into_iter()
                .flatten()
                .all(|p| p["memory"]["slug"] != "use-pnpm"
                    || p["rule"]["rule"] != "00000000000000a1"),
            "{again}"
        );

        // Wrong uses are refused with the way out.
        for (args, word) in [
            (
                json!({"slug": "use-pnpm", "decision": "supersedes", "rule": "00000000000000a1"}),
                "forget it",
            ),
            (
                json!({"slug": "use-pnpm", "decision": "conflict", "rule": "00000000000000a1", "other": "dark-theme"}),
                "not both",
            ),
            (
                json!({"slug": "use-pnpm", "decision": "conflict", "rule": "00000000000000ff"}),
                "conflicts() lists",
            ),
        ] {
            let mut full = args.clone();
            full["project"] = json!("/p");
            let err = call(&mut session, "settle", &full).unwrap_err();
            assert!(err.contains(word), "{word}: {err}");
        }
    }

    #[test]
    fn a_moved_line_keeps_its_decision_and_a_changed_or_removed_one_drops_it() {
        let mut session = conflict_store(&[
            (
                "use-pnpm",
                "project",
                "Install dependencies with pnpm, not npm",
            ),
            (
                "dark-theme",
                "global",
                "The user prefers a dark editor theme",
            ),
            ("small-commits", "project", "Keep commits small and focused"),
        ]);
        sync_rules(
            &mut session,
            vec![rule("00000000000000b1", NPM, "/p/CLAUDE.md", 3, false)],
        );
        call_in(
            &mut session,
            "settle",
            json!({"slug": "use-pnpm", "decision": "conflict", "rule": "00000000000000b1"}),
        );
        // Moved: the same id at another line.
        let moved = sync_rules(
            &mut session,
            vec![rule("00000000000000b1", NPM, "/p/CLAUDE.md", 9, false)],
        );
        assert_eq!(moved["updated"], 1, "{moved}");
        assert_eq!(moved["created"], 0);
        let listed = call_in(&mut session, "conflicts", json!({"k": 5}));
        assert_eq!(listed["rules_open"][0]["rule"]["line"], 9, "{listed}");
        // Changed text = a new id: the old line goes with its conflict, the new one is a candidate.
        let changed = sync_rules(
            &mut session,
            vec![rule(
                "00000000000000b2",
                "Always use pnpm for installing packages.",
                "/p/CLAUDE.md",
                9,
                false,
            )],
        );
        assert_eq!(changed["removed"], 1, "{changed}");
        let listed = call_in(&mut session, "conflicts", json!({"k": 5}));
        assert!(listed.get("rules_open").is_none(), "{listed}");
        assert_eq!(
            listed["rules_unchecked"][0]["rule"]["rule"], "00000000000000b2",
            "{listed}"
        );
        // Removed: nothing is left.
        let removed = sync_rules(&mut session, vec![]);
        assert_eq!(removed["removed"], 1, "{removed}");
        let listed = call_in(&mut session, "conflicts", json!({"k": 5}));
        assert!(listed.get("rules_unchecked").is_none(), "{listed}");
        assert_eq!(
            call_in(&mut session, "open_conflicts", json!({}))["rules_unchecked"],
            0
        );
    }

    #[test]
    fn a_changed_memory_is_checked_again_and_remember_names_the_lines_near_it() {
        let mut session = conflict_store(&[(
            "dark-theme",
            "global",
            "The user prefers a dark editor theme",
        )]);
        sync_rules(
            &mut session,
            vec![
                rule("00000000000000c1", NPM, "/p/CLAUDE.md", 3, false),
                rule(
                    "00000000000000c2",
                    "Answer in short sentences without filler words.",
                    "/home/u/.claude/CLAUDE.md",
                    1,
                    true,
                ),
            ],
        );
        // A new memory near a line: remember names it, and conflicts() lists it.
        let answer = remember_in(
            &mut session,
            json!({"slug": "use-pnpm", "summary": "Install dependencies with pnpm, not npm"}),
        );
        let lines = answer["instruction_lines"]
            .as_array()
            .unwrap_or_else(|| panic!("{answer}"));
        assert_eq!(lines[0]["rule"], "00000000000000c1", "{answer}");
        assert_eq!(lines[0]["file"], "/p/CLAUDE.md");
        assert!(
            lines.iter().all(|l| l["rule"] != "00000000000000c2"),
            "a project memory is not checked against a user-level line: {answer}"
        );
        assert!(
            answer["check_lines"]
                .as_str()
                .unwrap()
                .contains("settle(\"use-pnpm\", \"conflict\", rule: <rule>)")
        );
        call_in(
            &mut session,
            "settle",
            json!({"slug": "use-pnpm", "decision": "compatible", "rule": "00000000000000c1"}),
        );
        // The same text again keeps the decision.
        let same = remember_in(
            &mut session,
            json!({"slug": "use-pnpm", "summary": "Install dependencies with pnpm, not npm"}),
        );
        assert!(same.get("instruction_lines").is_none(), "{same}");
        let listed = call_in(&mut session, "conflicts", json!({"k": 5}));
        assert!(listed.get("rules_unchecked").is_none(), "{listed}");
        // A changed text is checked again.
        remember_in(
            &mut session,
            json!({"slug": "use-pnpm", "summary": "Install dependencies with pnpm only, never npm"}),
        );
        let listed = call_in(&mut session, "conflicts", json!({"k": 5}));
        assert_eq!(
            listed["rules_unchecked"][0]["memory"]["slug"], "use-pnpm",
            "{listed}"
        );
        // A forgotten memory takes its candidates with it.
        call_in(&mut session, "forget", json!({"slug": "use-pnpm"}));
        let listed = call_in(&mut session, "conflicts", json!({"k": 5}));
        assert!(listed.get("rules_unchecked").is_none(), "{listed}");
    }

    #[test]
    fn instruction_lines_are_never_found_as_memories() {
        let mut session =
            conflict_store(&[("use-pnpm", "project", "Install dependencies with pnpm")]);
        sync_rules(
            &mut session,
            vec![rule("00000000000000d1", NPM, "/p/CLAUDE.md", 3, false)],
        );
        let hits = call_in(
            &mut session,
            "recall",
            json!({"text": NPM, "k": 10, "label": ""}),
        );
        assert!(
            hits.as_array()
                .unwrap()
                .iter()
                .all(|h| h["label"] != "instruction"),
            "{hits}"
        );
        let found = call_in(
            &mut session,
            "memories",
            json!({"text": NPM, "k": 10, "kind": ""}),
        );
        assert!(!found.to_string().contains("instruction:"), "{found}");
    }
}
