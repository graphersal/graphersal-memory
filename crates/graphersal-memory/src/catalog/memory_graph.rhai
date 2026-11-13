fn memory_graph(project) {
    // The browser UI's Graph tab: this project's and the global memories (project "" = the
    // global ones only), the superseded ones included, with the decision edges among them:
    // supersedes (newer -> older), conflicts_with, narrows (project -> global) and relates_to.
    // Sessions and facts are not part of it (memory_neighbours adds those of one memory).
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    let rows = g.v().has_label("memory").filter(visible)
        .project(["id", "slug", "scope", "project", "kind", "title", "summary", "source", "marks", "updated_at", "sup", "conflicts"])
        .by(T.id).by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
        .by(__.coalesce(__.values("project"), __.constant("")))
        .by("kind").by(__.coalesce(__.values("title"), __.values("name"))).by("summary")
        .by(__.choose(__.has("imported_at"), __.coalesce(__.values("source"), __.constant("remember")), __.choose(__.has("source", "ui"), __.constant("ui"), __.constant("remember"))))
        .by(__.project("imported_at", "pinned", "unchecked")
            .by(__.coalesce(__.values("imported_at").as_string(), __.constant("")))
            .by(__.coalesce(__.values("pinned"), __.constant(false)))
            .by(__.coalesce(__.values("unchecked"), __.constant(false))))
        .by(__.coalesce(__.values("updated_at").as_string(), __.constant("")))
        .by(__.in_("supersedes").has_label("memory").filter(visible).values("name").fold())
        .by(__.both("conflicts_with").has_label("memory").filter(visible).values("name").dedup().fold()).to_list();
    let nodes = [];
    for r in rows {
        r.imported_at = r.marks.imported_at;
        r.pinned = r.marks.pinned;
        r.unchecked = r.marks.unchecked;
        r.remove("marks");
        r.source_kind = if r.imported_at != "" {
            if r.scope == "global" && (r.slug.starts_with("kg-") || r.slug.starts_with("basic-")) { "other" } else { "file" }
        } else if r.source == "ui" { "ui" } else { "agent" };
        if r.imported_at == "" { r.remove("imported_at"); }
        if r.sup.len() > 0 { r.superseded_by = r.sup[0]; }
        r.remove("sup");
        if r.conflicts.len() == 0 { r.remove("conflicts"); }
        if r.unchecked != true { r.remove("unchecked"); }
        r.always = !r.contains("superseded_by") && (r.pinned == true || r.kind == "feedback" || r.kind == "user");
        nodes.push(r);
    }
    let edges = g.v().has_label("memory").filter(visible)
        .out_e("supersedes", "conflicts_with", "narrows", "relates_to")
        .filter(__.in_v().has_label("memory").filter(visible))
        .project("id", "label", "source", "target", "at")
        .by(T.id).by(T.label).by(__.out_v().id()).by(__.in_v().id())
        .by(__.coalesce(__.values("at").as_string(), __.constant(""))).to_list();
    #{nodes: nodes, edges: edges}
}
