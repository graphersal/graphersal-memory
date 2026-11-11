fn memory(slug, project) {
    let s = if slug.starts_with("memory:") { slug.sub_string(7) } else { slug };
    // The current project's memory of that slug first, then the global one.
    let id = "";
    if project != "" && g.v("memory:" + s + "@" + project).has_label("memory").count().next() > 0 { id = "memory:" + s + "@" + project; }
    else if g.v("memory:" + s).has_label("memory").count().next() > 0 { id = "memory:" + s; }
    if id == "" { return #{error: "no memory " + s + " in this project or the global memory; list them with memories()"}; }
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    let m = g.v(id).project("slug", "scope", "kind", "title", "hook", "summary", "body", "updated_at", "source")
        .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
        .by("kind").by(__.coalesce(__.values("title"), __.values("name")))
        .by(__.coalesce(__.values("hook"), __.values("summary"))).by("summary")
        .by(__.coalesce(__.values("body"), __.constant(""))).by("updated_at")
        .by(__.choose(__.has("imported_at"), __.coalesce(__.values("source"), __.constant("remember")), __.constant("remember"))).next();
    let imported = g.v(id).values("imported_at").as_string().to_list();
    if imported.len() > 0 { m.imported_at = imported[0]; }
    if g.v(id).has("unchecked", true).count().next() > 0 { m.unchecked = true; }
    m.links = g.v(id).out("relates_to").has_label("memory").values("name").to_list();
    m.linked_from = g.v(id).in_("relates_to").has_label("memory").values("name").to_list();
    m.about = g.v(id).out("about").id().to_list();
    // The decision history: what supersedes this memory, what it supersedes, and the whole
    // chain from the oldest to the newest decision.
    let by = g.v(id).in_e("supersedes").filter(__.out_v().has_label("memory").filter(visible))
        .project("slug", "at").by(__.out_v().values("name"))
        .by(__.coalesce(__.values("at").as_string(), __.out_v().values("updated_at").as_string(), __.constant(""))).to_list();
    for i in 0..by.len() { if by[i].at.len() >= 10 { by[i].at = by[i].at.sub_string(0, 10); } }
    if by.len() > 0 {
        m.superseded_by = by;
        m.summary = "SUPERSEDED by " + by[0].slug + " on " + by[0].at + ": " + m.summary;
    }
    let older = g.v(id).out_e("supersedes").filter(__.in_v().has_label("memory"))
        .project("slug", "at").by(__.in_v().values("name"))
        .by(__.coalesce(__.values("at").as_string(), __.constant(""))).to_list();
    for i in 0..older.len() { if older[i].at.len() >= 10 { older[i].at = older[i].at.sub_string(0, 10); } }
    if older.len() > 0 { m.supersedes = older; }
    if by.len() > 0 || older.len() > 0 {
        let chain = [m.slug];
        let seen = [id];
        let at = id;
        for i in 0..20 {
            let prev = g.v(at).out("supersedes").has_label("memory").id().to_list();
            if prev.len() == 0 || seen.contains(prev[0]) { break; }
            seen.push(prev[0]);
            chain.insert(0, g.v(prev[0]).values("name").next());
            at = prev[0];
        }
        at = id;
        for i in 0..20 {
            let next = g.v(at).in_("supersedes").has_label("memory").filter(visible).id().to_list();
            if next.len() == 0 || seen.contains(next[0]) { break; }
            seen.push(next[0]);
            chain.push(g.v(next[0]).values("name").next());
            at = next[0];
        }
        m.chain = chain;
    }
    let conflicts = [];
    for d in ["out", "in"] {
        let edges = if d == "out" { g.v(id).out_e("conflicts_with") } else { g.v(id).in_e("conflicts_with") };
        let other = if d == "out" { __.in_v() } else { __.out_v() };
        let rows = edges.filter(other.has_label("memory").filter(visible))
            .project("id", "since").by(if d == "out" { __.in_v().id() } else { __.out_v().id() })
            .by(__.coalesce(__.values("at").as_string(), __.constant(""))).to_list();
        for r in rows {
            let c = g.v(r.id).project("slug", "scope", "summary", "source")
                .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global"))).by("summary")
                .by(__.choose(__.has("imported_at"), __.coalesce(__.values("source"), __.constant("remember")), __.constant("remember"))).next();
            c.since = if r.since.len() >= 10 { r.since.sub_string(0, 10) } else { r.since };
            conflicts.push(c);
        }
    }
    if conflicts.len() > 0 {
        m.conflicts_with = conflicts;
        m.settle = "An open conflict: ask the user which holds, then settle(<winner>, \"supersedes\", <loser>) or forget the loser; when both hold, settle(\"" + m.slug + "\", \"compatible\", <other>).";
    }
    let narrows = g.v(id).out("narrows").has_label("memory").values("name").to_list();
    if narrows.len() > 0 { m.narrows = narrows; m.note = "narrows the global rule " + narrows[0]; }
    let narrowed = g.v(id).in_("narrows").has_label("memory").filter(visible).values("name").to_list();
    if narrowed.len() > 0 { m.narrowed_by = narrowed; }
    m
}
