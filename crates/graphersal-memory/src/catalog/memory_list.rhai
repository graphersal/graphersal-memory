fn memory_list(project, kind, scope, source_kind, superseded, offset, limit) {
    // The browser UI's memory list: this project's and the global memories (project "" = the
    // global ones only) in the start context's order, filtered by kind, scope (project |
    // global), source_kind (agent | ui | file | other: a file of another memory tool) and,
    // unless superseded is true, without the superseded ones; one page from offset.
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    let t = g.v().has_label("memory").filter(visible);
    if kind != "" { t = t.has("kind", kind); }
    if scope == "project" { t = t.has("project"); }
    if scope == "global" { t = t.not(__.has("project")); }
    let group = __.choose(__.has("pinned", true), __.constant(0), __.choose(__.has("kind", "feedback"), __.constant(1),
            __.choose(__.has("kind", "user"), __.constant(2), __.constant(3))));
    let rows = t.order()
        .by(group)
        .by(__.coalesce(__.values("order"), __.constant(1000000)))
        .by(__.choose(__.or(__.has("pinned", true), __.has("kind", within(["feedback", "user"]))),
            __.coalesce(__.values("updated_at").as_string(), __.constant("")), __.constant("")), Order.desc)
        .by(__.coalesce(__.values("index_order"), __.constant(1000000))).by("name")
        .project(["slug", "scope", "project", "kind", "title", "hook", "summary", "source", "marks",
                  "updated_at", "sup", "conflicts"])
        .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
        .by(__.coalesce(__.values("project"), __.constant("")))
        .by("kind").by(__.coalesce(__.values("title"), __.values("name")))
        .by(__.coalesce(__.values("hook"), __.values("summary"))).by("summary")
        .by(__.choose(__.has("imported_at"), __.coalesce(__.values("source"), __.constant("remember")), __.choose(__.has("source", "ui"), __.constant("ui"), __.constant("remember"))))
        .by(__.project("imported_at", "pinned", "order", "unchecked", "edited_from")
            .by(__.coalesce(__.values("imported_at").as_string(), __.constant("")))
            .by(__.coalesce(__.values("pinned"), __.constant(false)))
            .by(__.values("order").fold())
            .by(__.coalesce(__.values("unchecked"), __.constant(false)))
            .by(__.coalesce(__.values("edited_from"), __.constant(""))))
        .by(__.coalesce(__.values("updated_at").as_string(), __.constant("")))
        .by(__.in_("supersedes").has_label("memory").filter(visible).values("name").fold())
        .by(__.both("conflicts_with").has_label("memory").filter(visible).values("name").fold()).to_list();
    let out = [];
    for r in rows {
        r.imported_at = r.marks.imported_at;
        r.pinned = r.marks.pinned;
        r.order = r.marks.order;
        r.unchecked = r.marks.unchecked;
        r.edited_from = r.marks.edited_from;
        r.remove("marks");
        let sk = if r.imported_at != "" {
            if r.scope == "global" && (r.slug.starts_with("kg-") || r.slug.starts_with("basic-")) { "other" } else { "file" }
        } else if r.source == "ui" { "ui" } else { "agent" };
        if source_kind != "" && sk != source_kind { continue; }
        if !superseded && r.sup.len() > 0 { continue; }
        r.source_kind = sk;
        r.order = if r.order.len() > 0 { r.order[0] } else { () };
        if r.imported_at == "" { r.remove("imported_at"); }
        if r.edited_from == "" { r.remove("edited_from"); }
        if r.sup.len() > 0 { r.superseded_by = r.sup[0]; }
        r.remove("sup");
        if r.conflicts.len() == 0 { r.remove("conflicts"); }
        if r.unchecked != true { r.remove("unchecked"); }
        r.always = !r.contains("superseded_by") && (r.pinned == true || r.kind == "feedback" || r.kind == "user");
        out.push(r);
    }
    let total = out.len();
    let page = [];
    for i in offset..total {
        if page.len() >= limit { break; }
        page.push(out[i]);
    }
    #{total: total, offset: offset, limit: limit, rows: page}
}
