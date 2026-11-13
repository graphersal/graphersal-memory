fn memory_neighbours(slug, project) {
    // The browser UI's Graph tab, a memory expanded: the sessions and facts around the memory
    // <slug> (this project's first, else the global one): the sessions that `wrote` it (newest
    // first, at most 30), what it is `about` and what is about it, the session of each such
    // fact, the facts of each such session (at most 30), what a session `edited` and the other
    // memories it `wrote` (at most 30 each), with the wrote / about / contains / edited edges
    // among them and the memory. Other memories are not repeated (the graph has them); an edge
    // may end at one. `imported`: the file of a memory imported from the harness's file memory
    // (no session wrote it).
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    let id = "";
    if project != "" && g.v("memory:" + slug + "@" + project).has_label("memory").count().next() > 0 { id = "memory:" + slug + "@" + project; }
    else if g.v("memory:" + slug).has_label("memory").count().next() > 0 { id = "memory:" + slug; }
    if id == "" { return #{error: "no memory " + slug + " in this project or the global memory"}; }
    let near = g.v(id).in_("wrote").has_label("session").filter(visible)
        .order().by(__.coalesce(__.values("started_at").as_string(), __.constant("")), Order.desc)
        .limit(30).id().to_list();
    for n in g.v(id).both("about").filter(visible).dedup().id().to_list() {
        if !near.contains(n) { near.push(n); }
    }
    let ids = [id];
    for n in near {
        if !ids.contains(n) { ids.push(n); }
        let label = g.v(n).label().next();
        let more = [];
        if label == "fact" {
            more = g.v(n).in_("contains").has_label("session").filter(visible).id().to_list();
        } else if label == "session" {
            more = g.v(n).out("contains").has_label("fact").filter(visible).limit(30).id().to_list();
            more += g.v(n).out("edited").filter(visible).limit(30).id().to_list();
            more += g.v(n).out("wrote").has_label("memory").filter(visible).limit(30).id().to_list();
        }
        for m in more { if !ids.contains(m) { ids.push(m); } }
    }
    let rows = g.v(ids).not(__.has_label("memory"))
        .project(["id", "label", "name", "summary", "project", "fact_kind", "why", "at", "started_at", "harness", "turns", "session", "headline"])
        .by(T.id).by(T.label).by(__.coalesce(__.values("name"), __.constant("")))
        .by(__.coalesce(__.values("summary"), __.constant("")))
        .by(__.coalesce(__.values("project"), __.constant("")))
        .by(__.coalesce(__.values("fact_kind"), __.constant("")))
        .by(__.coalesce(__.values("why"), __.constant("")))
        .by(__.coalesce(__.values("at").as_string(), __.constant("")))
        .by(__.coalesce(__.values("started_at").as_string(), __.constant("")))
        .by(__.coalesce(__.values("harness"), __.constant("")))
        .by(__.coalesce(__.values("turns"), __.constant(0)))
        .by(__.coalesce(__.in_("contains").has_label("session").id(), __.constant("")))
        .by(__.coalesce(__.in_("contains").has_label("session").values("name"), __.constant(""))).to_list();
    let edges = g.v(ids).out_e("wrote", "about", "contains", "edited").filter(__.in_v().has_id(within(ids)))
        .project("id", "label", "source", "target", "actions", "at")
        .by(T.id).by(T.label).by(__.out_v().id()).by(__.in_v().id())
        .by(__.coalesce(__.values("actions"), __.constant([])))
        .by(__.coalesce(__.values("at"), __.constant(""))).to_list();
    // The memories an edge reaches are the graph's already: the other ends of about edges.
    let memories = g.v(ids).has_label("memory").id().to_list();
    let imported = g.v(id).has("imported_at").coalesce(__.values("source"), __.constant("")).to_list();
    #{memory: id, nodes: rows, edges: edges, memories: memories,
      imported: if imported.len() > 0 { imported[0] } else { "" }}
}
