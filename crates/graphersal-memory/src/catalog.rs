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
pub const INTERNAL_FUNCTIONS: [&str; 8] = [
    "indexed_sessions",
    "index_session",
    "index_turns",
    "set_summary",
    "merge_project",
    "export_memories",
    "start_context",
    "open_conflicts",
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
        "indexed_sessions" => include_str!("catalog/indexed_sessions.rhai"),
        "index_session" => include_str!("catalog/index_session.rhai"),
        "index_turns" => include_str!("catalog/index_turns.rhai"),
        "set_summary" => include_str!("catalog/set_summary.rhai"),
        "merge_project" => include_str!("catalog/merge_project.rhai"),
        "export_memories" => include_str!("catalog/export_memories.rhai"),
        "start_context" => include_str!("catalog/start_context.rhai"),
        "open_conflicts" => include_str!("catalog/open_conflicts.rhai"),
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
    fn the_fingerprint_is_stable_and_marks_product_queries() {
        assert_eq!(fingerprint(), fingerprint());
        assert_eq!(fingerprint().len(), 16);
        assert_eq!(product_fingerprint(&json!(meta())), Some(fingerprint()));
        assert_eq!(product_fingerprint(&json!("{\"origin\":\"user\"}")), None);
        assert_eq!(product_fingerprint(&Value::Null), None);
    }
}
