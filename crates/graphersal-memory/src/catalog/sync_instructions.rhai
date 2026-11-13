fn sync_instructions(project, rules) {
    // The rules of the instruction files the harnesses load (crate::instructions: one list
    // item, paragraph or line each, {id, text, file, line, loaded_by, global}) become
    // `instruction` vertices of this project (a user-level file's: global ones). A rule is
    // known by its id (a fingerprint of the project, its file and its text): a moved line only
    // updates `line`; a changed or removed one is dropped with its candidates and decisions
    // (project "" = the global rules only).
    let mine = if project == "" { __.not(__.has("project")) }
               else { __.or(__.not(__.has("project")), __.has("project", project)) };
    let existing = #{};
    for row in g.v().has_label("instruction").filter(mine)
        .project("id", "line", "file", "loaded").by(T.id).by("line").by("file")
        .by(__.coalesce(__.values("loaded_by"), __.constant(""))).to_list() {
        existing[row.id] = row;
    }
    let now = g.gql("RETURN CURRENT_TIMESTAMP AS t").next()["t"];
    let seen = #{};
    let created = 0;
    let updated = 0;
    for r in rules {
        if type_of(r.id) != "string" || !r.id.starts_with("instruction:") || type_of(r.text) != "string" { continue; }
        let global = r.global == true;
        if !global && project == "" { continue; }
        seen[r.id] = true;
        if existing.contains(r.id) {
            let e = existing[r.id];
            if e.line != r.line || e.file != r.file || e.loaded != r.loaded_by {
                g.v(r.id).property("line", r.line).property("file", r.file).property("source", r.file)
                    .property("loaded_by", r.loaded_by).iterate();
                updated += 1;
            }
        } else {
            let p = #{text: r.text, file: r.file, source: r.file, line: r.line, loaded_by: r.loaded_by,
                      origin: "instructions", visibility: "private", updated_at: now, embed_text: r.text,
                      scanned: false};
            if !global { p.project = project; }
            g.merge_v(#{"T.id": r.id, "T.label": "instruction"}).option(Merge.onCreate, p).option(Merge.onMatch, p).iterate();
            created += 1;
        }
    }
    let removed = 0;
    for id in existing.keys() {
        if !seen.contains(id) {
            g.v(id).drop().iterate();
            removed += 1;
        }
    }
    let pending = g.v().has_label("instruction").filter(mine).has("scanned", false).count().next();
    #{rules: seen.len(), created: created, updated: updated, removed: removed, pending: pending}
}
