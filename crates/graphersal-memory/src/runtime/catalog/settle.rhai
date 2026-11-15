fn settle(slug, decision, other, rule, project) {
    // The current project's memory of a slug first, then the global one.
    let id = "";
    if project != "" && g.v("memory:" + slug + "@" + project).has_label("memory").count().next() > 0 { id = "memory:" + slug + "@" + project; }
    else if g.v("memory:" + slug).has_label("memory").count().next() > 0 { id = "memory:" + slug; }
    if id == "" { return #{error: "no memory " + slug + " in this project or the global memory; list them with memories()"}; }
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    // rule: a line of an instruction file (conflicts() lists its id) instead of another memory.
    if rule != "" {
        if other != "" { return #{error: "settle a memory against another memory (other) or a line of an instruction file (rule), not both"}; }
        if decision != "compatible" && decision != "conflict" {
            return #{error: "against a line of an instruction file the decision is compatible or conflict; when the memory is outdated, forget it (or remember the new rule with supersedes: [" + slug + "]); when the file is outdated, the user edits that line"};
        }
        let rid = "instruction:" + rule;
        if g.v(rid).has_label("instruction").filter(visible).count().next() == 0 {
            return #{error: "no line " + rule + " of an instruction file in this project; conflicts() lists them"};
        }
        let at = g.gql("RETURN CURRENT_TIMESTAMP AS t").next()["t"];
        g.v(rid).out_e("compared_with").filter(__.in_v().has_id(id)).drop().iterate();
        g.merge_e(#{"T.label": "compared_with", "Direction.OUT": rid, "Direction.IN": id})
            .option(Merge.onCreate, #{origin: "curated", source: "derived", visibility: "private", decision: decision, at: at}).iterate();
        let r = g.v(rid).project("file", "line").by("file").by("line").next();
        let out = #{slug: slug, decision: decision, rule: rule, file: r.file, line: r.line};
        if decision == "conflict" {
            let memory_file = "";
            if g.v(id).has("imported_at").count().next() > 0 { memory_file = g.v(id).values("source").next(); }
            out.note = "Ask the user which holds. If the memory is outdated: forget it, or remember the new rule with supersedes: [" + slug + "]"
                + (if memory_file != "" { " (it was imported from " + memory_file + ": the user edits or deletes that file too)" } else { "" })
                + ". If the file is outdated: the user edits " + r.file + " line " + r.line + " (graphersal-memory never edits instruction files); the conflict closes when that line changes or goes.";
        }
        out.rule_conflicts = g.v().has_label("instruction").filter(visible).out_e("compared_with").has("decision", "conflict").filter(__.in_v().filter(visible)).count().next();
        out.rules_unchecked = g.v().has_label("instruction").filter(visible).out_e("compared_with").has("decision", "unchecked").filter(__.in_v().filter(visible)).count().next();
        return out;
    }
    let oid = "";
    if other != "" {
        if project != "" && g.v("memory:" + other + "@" + project).has_label("memory").count().next() > 0 { oid = "memory:" + other + "@" + project; }
        else if g.v("memory:" + other).has_label("memory").count().next() > 0 { oid = "memory:" + other; }
        if oid == "" { return #{error: "no memory " + other + " in this project or the global memory; list them with memories()"}; }
        if oid == id { return #{error: "settle a memory against another one, not itself"}; }
    } else if decision != "compatible" {
        return #{error: "the decision " + decision + " needs the other memory: settle(slug, decision, other)"};
    }
    let now = g.gql("RETURN CURRENT_TIMESTAMP AS t").next()["t"];
    let scope = if id.contains("@") { "project" } else { "global" };
    let other_scope = if oid.contains("@") { "project" } else { "global" };
    let e = #{origin: "curated", source: "derived", visibility: "private", at: now};
    let out = #{slug: slug, decision: decision};
    if other != "" { out.other = other; }
    if decision == "conflict" {
        // A project memory that differs from a global one narrows it: the project's holds there.
        if scope != other_scope {
            return #{error: "a project memory and a global one do not conflict: the project's holds in this project; record it with settle(<project slug>, \"narrows\", <global slug>)"};
        }
        g.merge_e(#{"T.label": "conflicts_with", "Direction.OUT": id, "Direction.IN": oid}).option(Merge.onCreate, e).iterate();
        let files = [];
        for x in [id, oid] {
            if g.v(x).has("imported_at").count().next() > 0 {
                files.push(#{slug: g.v(x).values("name").next(), file: g.v(x).values("source").next()});
            }
        }
        if files.len() > 0 {
            out.edit_files = files;
            out.note = "When the user has decided, the file of the memory that loses must change too, or its old rule stays in what the harness loads from there.";
        }
    } else {
        if oid != "" {
            // Every other decision ends an open conflict between the two.
            g.v(id).out_e("conflicts_with").filter(__.in_v().has_id(oid)).drop().iterate();
            g.v(id).in_e("conflicts_with").filter(__.out_v().has_id(oid)).drop().iterate();
        }
        if decision == "narrows" {
            if scope != "project" || other_scope != "global" {
                return #{error: "narrows links a project memory to the global memory it narrows: settle(<project slug>, \"narrows\", <global slug>)"};
            }
            g.merge_e(#{"T.label": "narrows", "Direction.OUT": id, "Direction.IN": oid}).option(Merge.onCreate, e).iterate();
            out.note = slug + " narrows the global rule " + other;
        } else if decision == "supersedes" {
            g.merge_e(#{"T.label": "supersedes", "Direction.OUT": id, "Direction.IN": oid}).option(Merge.onCreate, e).iterate();
            // A superseded memory is settled: its open conflicts end with it.
            g.v(oid).both_e("conflicts_with").drop().iterate();
            g.v(oid).properties("unchecked").drop().iterate();
            if g.v(oid).has("imported_at").count().next() > 0 {
                out.edit_files = [#{slug: other, file: g.v(oid).values("source").next()}];
                out.note = "The superseded memory came from this file: tell the user to edit or delete it too, or the old rule stays in what the harness loads from there.";
            }
        }
    }
    // The memory is checked: an import's mark goes.
    g.v(id).properties("unchecked").drop().iterate();
    out.open_conflicts = g.v().has_label("memory").filter(visible).out_e("conflicts_with").filter(__.in_v().filter(visible)).count().next();
    out.unchecked = g.v().has_label("memory").filter(visible).has("unchecked", true).count().next();
    out
}
