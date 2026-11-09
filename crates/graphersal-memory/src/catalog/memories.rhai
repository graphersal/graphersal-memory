fn memories(text, k, kind, project) {
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    if text == "" {
        let t = g.v().has_label("memory").filter(visible);
        if kind != "" { t = t.has("kind", kind); }
        return t.order().by(__.coalesce(__.values("index_order"), __.constant(1000000))).limit(k)
            .project("slug", "scope", "kind", "title", "hook", "updated_at")
            .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
            .by("kind").by(__.coalesce(__.values("title"), __.values("name")))
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
