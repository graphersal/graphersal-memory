fn merge_project(from, into) {
    // Every vertex of project `from` becomes one of project `into`. Memories carry the project
    // in their id: each is written again under its new id (properties and edges copied; when
    // `into` has a memory of the same slug, the more recently updated one stays), then the old
    // one is dropped. Sessions, turns and facts only change their `project`.
    let keys = ["name", "kind", "summary", "body", "embed_text", "origin", "visibility", "source",
                "updated_at", "why", "how_to_apply", "title", "hook", "index_order", "pinned", "order",
                "imported_at", "unchecked"];
    let renamed = #{};
    let moved = 0;
    let replaced = 0;
    let kept = 0;
    for id in g.v().has_label("memory").has("project", from).id().to_list() {
        let slug = g.v(id).values("name").next();
        let target = "memory:" + slug + "@" + into;
        if g.v(target).count().next() > 0 {
            let newest = g.v(id, target).order().by(__.coalesce(__.values("updated_at").as_string(), __.constant("")), Order.desc).id().limit(1).next();
            if newest == target {
                kept += 1;
                renamed[id] = "";
                continue;
            }
            g.v(target).drop().iterate();
            replaced += 1;
        }
        let p = #{project: into};
        for k in keys {
            let v = g.v(id).values(k).to_list();
            if v.len() > 0 { p[k] = v[0]; }
        }
        g.merge_v(#{"T.id": target, "T.label": "memory"}).option(Merge.onCreate, p).option(Merge.onMatch, p).iterate();
        renamed[id] = target;
        moved += 1;
    }
    // The edges of the renamed memories, both ways, to the new ids.
    for id in renamed.keys() {
        let target = renamed[id];
        if target == "" { continue; }
        let out = g.v(id).out_e().project("l", "to", "o", "s", "v", "at").by(T.label).by(__.in_v().id())
            .by(__.coalesce(__.values("origin"), __.constant(""))).by(__.coalesce(__.values("source"), __.constant("")))
            .by(__.coalesce(__.values("visibility"), __.constant("private"))).by(__.values("at").fold()).to_list();
        for e in out {
            let to = if renamed.contains(e.to) { renamed[e.to] } else { e.to };
            if to == "" || to == target { continue; }
            let ep = #{origin: e.o, source: e.s, visibility: e.v};
            if e.at.len() > 0 { ep.at = e.at[0]; }
            g.merge_e(#{"T.label": e.l, "Direction.OUT": target, "Direction.IN": to})
                .option(Merge.onCreate, ep).iterate();
        }
        let inc = g.v(id).in_e().project("l", "from", "o", "s", "v", "at").by(T.label).by(__.out_v().id())
            .by(__.coalesce(__.values("origin"), __.constant(""))).by(__.coalesce(__.values("source"), __.constant("")))
            .by(__.coalesce(__.values("visibility"), __.constant("private"))).by(__.values("at").fold()).to_list();
        for e in inc {
            if renamed.contains(e.from) { continue; }
            let ep = #{origin: e.o, source: e.s, visibility: e.v};
            if e.at.len() > 0 { ep.at = e.at[0]; }
            g.merge_e(#{"T.label": e.l, "Direction.OUT": e.from, "Direction.IN": target})
                .option(Merge.onCreate, ep).iterate();
        }
    }
    for id in renamed.keys() { g.v(id).drop().iterate(); }
    let others = g.v().has("project", from).count().next();
    g.v().has("project", from).property("project", into).iterate();
    #{from: from, into: into, memories_moved: moved, memories_replaced: replaced, memories_kept: kept, others: others}
}
