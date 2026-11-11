fn remember(slug, kind, summary, body, why, how_to_apply, title, hook, pinned, links, about, scope, project) {
    // A project memory needs a project; without one (no project known) it is global.
    let scoped = scope == "project" && project != "";
    let id = if scoped { "memory:" + slug + "@" + project } else { "memory:" + slug };
    let other = if scoped { "memory:" + slug } else if project != "" { "memory:" + slug + "@" + project } else { "" };
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    let now = g.gql("RETURN CURRENT_TIMESTAMP AS t").next()["t"];
    let text = body;
    text.trim();
    if why != "" { text += "\n\n**Why:** " + why; }
    if how_to_apply != "" { text += (if why == "" { "\n\n" } else { "\n" }) + "**How to apply:** " + how_to_apply; }
    let p = #{name: slug, kind: kind, summary: summary, body: text, embed_text: slug + ": " + summary,
              origin: "curated", visibility: "private", source: "user", updated_at: now};
    if scoped { p.project = project; }
    if why != "" { p.why = why; }
    if how_to_apply != "" { p.how_to_apply = how_to_apply; }
    if title != "" { p.title = title; }
    if hook != "" { p.hook = hook; }
    // pinned: true/false sets it; null keeps what the memory has (pinned and order survive a replace).
    if type_of(pinned) == "bool" { p.pinned = pinned; }
    let created = g.v(id).count().next() == 0;
    if created {
        let mx = g.v().has_label("memory").values("index_order").max().to_list();
        p.index_order = if mx.len() == 0 { 0 } else { mx[0] + 1 };
    } else {
        // A replaced memory keeps no optional field of its old text.
        g.v(id).properties("why", "how_to_apply", "title", "hook").drop().iterate();
    }
    g.merge_v(#{"T.id": id, "T.label": "memory"}).option(Merge.onCreate, p).option(Merge.onMatch, p).iterate();
    let missing = [];
    let e = #{origin: "curated", source: "derived", visibility: "private"};
    for l in links {
        let target = "";
        if project != "" && g.v("memory:" + l + "@" + project).count().next() > 0 { target = "memory:" + l + "@" + project; }
        else if g.v("memory:" + l).count().next() > 0 { target = "memory:" + l; }
        if target != "" {
            g.merge_e(#{"T.label": "relates_to", "Direction.OUT": id, "Direction.IN": target}).option(Merge.onCreate, e).iterate();
        } else { missing.push(l); }
    }
    for a in about {
        if g.v(a).filter(visible).count().next() > 0 {
            g.merge_e(#{"T.label": "about", "Direction.OUT": id, "Direction.IN": a}).option(Merge.onCreate, e).iterate();
        } else { missing.push(a); }
    }
    let out = #{slug: slug, scope: if scoped { "project" } else { "global" }, created: created, missing_targets: missing};
    if type_of(pinned) == "bool" { out.pinned = pinned; }
    if other != "" && g.v(other).count().next() > 0 { out.also_in = if scoped { "global" } else { "project" }; }
    out
}
