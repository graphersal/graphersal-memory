fn memories(text, k, kind, project) {
    if kind != "" && !["user", "feedback", "project", "reference"].contains(kind) {
        return #{error: "unknown kind " + kind + ": use user, feedback, project or reference (empty: every kind)"};
    }
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    if text == "" {
        let t = g.v().has_label("memory").filter(visible);
        if kind != "" { t = t.has("kind", kind); }
        // The start context's order: pinned, feedback, user (each by order, then newest), then
        // the rest by order, then list position (as imported from MEMORY.md, then as written).
        let group = __.choose(__.has("pinned", true), __.constant(0), __.choose(__.has("kind", "feedback"), __.constant(1),
                __.choose(__.has("kind", "user"), __.constant(2), __.constant(3))));
        let rows = t.order()
            .by(group)
            .by(__.coalesce(__.values("order"), __.constant(1000000)))
            .by(__.choose(__.or(__.has("pinned", true), __.has("kind", within(["feedback", "user"]))),
                __.coalesce(__.values("updated_at").as_string(), __.constant("")), __.constant("")), Order.desc)
            .by(__.coalesce(__.values("index_order"), __.constant(1000000))).by("name").limit(k)
            .project("slug", "scope", "kind", "pinned", "title", "hook", "updated_at", "sup", "conflicts")
            .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
            .by("kind").by(__.coalesce(__.values("pinned"), __.constant(false))).by(__.coalesce(__.values("title"), __.values("name")))
            .by(__.coalesce(__.values("hook"), __.values("summary"))).by("updated_at")
            .by(__.in_e("supersedes").filter(__.out_v().has_label("memory").filter(visible))
                .project("slug", "at").by(__.out_v().values("name"))
                .by(__.coalesce(__.values("at").as_string(), __.out_v().values("updated_at").as_string(), __.constant(""))).fold())
            .by(__.both("conflicts_with").has_label("memory").filter(visible).values("name").fold()).to_list();
        // A superseded memory stays listed, marked; an open conflict names the other memory.
        for i in 0..rows.len() {
            let sup = rows[i].sup;
            if sup.len() > 0 {
                let at = if sup[0].at.len() >= 10 { sup[0].at.sub_string(0, 10) } else { sup[0].at };
                rows[i].hook = "SUPERSEDED by " + sup[0].slug + " on " + at + ": " + rows[i].hook;
                rows[i].superseded_by = sup[0].slug;
            }
            if rows[i].conflicts.len() == 0 { rows[i].remove("conflicts"); }
            else { rows[i].open_conflict_with = rows[i].conflicts; rows[i].remove("conflicts"); }
            rows[i].remove("sup");
        }
        return rows;
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
            // A superseded memory is marked (it leads to its successor); an open conflict shows
            // the other version.
            let sup = g.v(h.id).in_e("supersedes").filter(__.out_v().has_label("memory").filter(visible))
                .project("slug", "at").by(__.out_v().values("name"))
                .by(__.coalesce(__.values("at").as_string(), __.out_v().values("updated_at").as_string(), __.constant(""))).to_list();
            if sup.len() > 0 {
                let at = if sup[0].at.len() >= 10 { sup[0].at.sub_string(0, 10) } else { sup[0].at };
                m.summary = "SUPERSEDED by " + sup[0].slug + " on " + at + ": " + m.summary;
                m.superseded_by = sup[0].slug;
            }
            let c = g.v(h.id).both("conflicts_with").has_label("memory").filter(visible)
                .project("slug", "scope", "summary").by("name")
                .by(__.coalesce(__.values("project").constant("project"), __.constant("global"))).by("summary").to_list();
            if c.len() > 0 { m.open_conflict_with = c; }
            out.push(m);
        }
    }
    out
}
