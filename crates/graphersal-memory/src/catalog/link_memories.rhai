fn link_memories(id, links) {
    // The session indexing: the memories the session <id> wrote through the memory tools
    // (`links`: [{slug, actions, at, last_at, count}], one per memory, from its transcript)
    // become `wrote` edges session -> memory: the session's project's memory of that slug first,
    // then the global one. Every call replaces the session's wrote edges as a whole, so a
    // repeated call (a re-index, the one-time backfill) changes nothing. A slug without a memory
    // (forgotten since, or a call that failed) is answered in `missing`.
    if g.v(id).has_label("session").count().next() == 0 { return #{error: "no session " + id}; }
    let project = g.v(id).coalesce(__.values("project"), __.constant("")).next();
    g.v(id).out_e("wrote").drop().iterate();
    let linked = [];
    let missing = [];
    for l in links {
        let slug = l.slug;
        let mid = "";
        if project != "" && g.v("memory:" + slug + "@" + project).has_label("memory").count().next() > 0 { mid = "memory:" + slug + "@" + project; }
        else if g.v("memory:" + slug).has_label("memory").count().next() > 0 { mid = "memory:" + slug; }
        if mid == "" { missing.push(slug); continue; }
        if linked.contains(mid) { continue; }
        let p = #{origin: "sessions", source: "derived", visibility: "private",
                  actions: l.actions, at: l.at, last_at: l.last_at, count: l.count};
        g.merge_e(#{"T.label": "wrote", "Direction.OUT": id, "Direction.IN": mid}).option(Merge.onCreate, p).option(Merge.onMatch, p).iterate();
        linked.push(mid);
    }
    #{linked: linked, missing: missing}
}
