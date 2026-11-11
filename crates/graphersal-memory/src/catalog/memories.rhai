fn memories(text, k, kind, project) {
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    if text == "" {
        let t = g.v().has_label("memory").filter(visible);
        if kind != "" { t = t.has("kind", kind); }
        // The start context's order: pinned, feedback, user (each by order, then newest), then
        // the rest by order, then list position (as imported from MEMORY.md, then as written).
        let group = __.choose(__.has("pinned", true), __.constant(0), __.choose(__.has("kind", "feedback"), __.constant(1),
                __.choose(__.has("kind", "user"), __.constant(2), __.constant(3))));
        return t.order()
            .by(group)
            .by(__.coalesce(__.values("order"), __.constant(1000000)))
            .by(__.choose(__.or(__.has("pinned", true), __.has("kind", within(["feedback", "user"]))),
                __.coalesce(__.values("updated_at").as_string(), __.constant("")), __.constant("")), Order.desc)
            .by(__.coalesce(__.values("index_order"), __.constant(1000000))).by("name").limit(k)
            .project("slug", "scope", "kind", "pinned", "title", "hook", "updated_at")
            .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
            .by("kind").by(__.coalesce(__.values("pinned"), __.constant(false))).by(__.coalesce(__.values("title"), __.values("name")))
            .by(__.coalesce(__.values("hook"), __.values("summary"))).by("updated_at").to_list();
    }
    let hits = g.query("recall", #{text: text, k: if kind == "" { k } else { k * 4 }, label: "memory", project: project});
    let out = [];
    for h in hits {
        if out.len() >= k { break; }
        let m = g.v(h.id).project("slug", "scope", "kind", "title", "summary", "score")
            .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
            .by("kind").by(__.coalesce(__.values("title"), __.values("name"))).by("summary")
            .by(__.constant(h.score)).next();
        if kind == "" || m.kind == kind {
            if out.len() < 2 { m.body = g.v(h.id).coalesce(__.values("body"), __.constant("")).next(); }
            out.push(m);
        }
    }
    out
}
