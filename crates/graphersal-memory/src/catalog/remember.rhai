fn remember(slug, kind, summary, body, why, how_to_apply, title, hook, pinned, order, links, about, supersedes, from_file, scope, project) {
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
    // from_file (the file-memory import only): the file it came from and when that file last
    // changed (the decision time); imported_at is when it was imported.
    let imported = false;
    if type_of(from_file) == "map" && from_file.contains("path") && type_of(from_file.path) == "string" && from_file.path != "" {
        imported = true;
        p.source = from_file.path;
        p.imported_at = now;
        if from_file.contains("modified_at") && type_of(from_file.modified_at) == "string" && from_file.modified_at != "" {
            p.updated_at = from_file.modified_at;
        }
    }
    if scoped { p.project = project; }
    if why != "" { p.why = why; }
    if how_to_apply != "" { p.how_to_apply = how_to_apply; }
    if title != "" { p.title = title; }
    if hook != "" { p.hook = hook; }
    // pinned: true/false sets it; null keeps what the memory has (pinned and order survive a replace).
    if type_of(pinned) == "bool" { p.pinned = pinned; }
    // order: a number sets the priority within its group; null keeps what the memory has.
    if type_of(order) == "i64" { p.order = order; }
    let created = g.v(id).count().next() == 0;
    if created {
        let mx = g.v().has_label("memory").values("index_order").max().to_list();
        p.index_order = if mx.len() == 0 { 0 } else { mx[0] + 1 };
    } else {
        // A replaced memory keeps no optional field of its old text, nor an import's marks when
        // the agent writes it.
        g.v(id).properties("why", "how_to_apply", "title", "hook").drop().iterate();
        if !imported { g.v(id).properties("imported_at", "unchecked").drop().iterate(); }
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
    // supersedes: the older memories this one replaces as a later decision. They stay, marked
    // SUPERSEDED in every search; their open conflicts are settled by it.
    let replaced = [];
    let superseded = [];
    let files = [];
    for s in supersedes {
        let target = "";
        if project != "" && g.v("memory:" + s + "@" + project).has_label("memory").count().next() > 0 { target = "memory:" + s + "@" + project; }
        else if g.v("memory:" + s).has_label("memory").count().next() > 0 { target = "memory:" + s; }
        if target == "" || target == id { missing.push(s); continue; }
        g.merge_e(#{"T.label": "supersedes", "Direction.OUT": id, "Direction.IN": target})
            .option(Merge.onCreate, #{origin: "curated", source: "derived", visibility: "private", at: now}).iterate();
        // A superseded memory is settled: its open conflicts end with it.
        g.v(target).both_e("conflicts_with").drop().iterate();
        g.v(target).properties("unchecked").drop().iterate();
        replaced.push(target);
        superseded.push(s);
        if g.v(target).has("imported_at").count().next() > 0 {
            files.push(#{slug: s, file: g.v(target).values("source").next()});
        }
    }
    let out = #{slug: slug, scope: if scoped { "project" } else { "global" }, created: created, missing_targets: missing};
    if type_of(pinned) == "bool" { out.pinned = pinned; }
    if other != "" && g.v(other).count().next() > 0 { out.also_in = if scoped { "global" } else { "project" }; }
    if superseded.len() > 0 { out.superseded = superseded; }
    if files.len() > 0 {
        out.edit_files = files;
        out.note = "The superseded memories came from these files: tell the user to edit or delete them too, or the old rule stays in what the harness loads from there.";
    }
    // The nearest existing memories for the conflict check the agent makes: the recall scoring
    // on this memory's text over this project's and the global memories, without itself, the
    // ones it supersedes and the ones already superseded.
    let hits = g.query("recall", #{text: slug + ": " + summary, k: 12, label: "memory", project: project});
    let nearest = [];
    for h in hits {
        if nearest.len() >= 5 { break; }
        if h.id == id || replaced.contains(h.id) { continue; }
        if g.v(h.id).in_("supersedes").has_label("memory").filter(visible).count().next() > 0 { continue; }
        nearest.push(g.v(h.id).project("slug", "scope", "kind", "summary", "source", "updated_at")
            .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
            .by("kind").by("summary")
            .by(__.choose(__.has("imported_at"), __.coalesce(__.values("source"), __.constant("remember")), __.constant("remember")))
            .by(__.coalesce(__.values("updated_at").as_string(), __.constant(""))).next());
    }
    if nearest.len() > 0 {
        out.nearest = nearest;
        out.check = "Check the new memory " + slug + " against each nearest memory below; most are merely on a similar topic. Pick ONE per memory:\n"
            + "- compatible: both can hold at the same time (different subject, scope, folder, tool or an explicit exception). This is the default. A project memory that narrows a global one is compatible, not a conflict (conflicts exist only within one scope): note it with settle(\"" + slug + "\", \"narrows\", <global slug>).\n"
            + "- replaces: it says the same thing as the new memory in other words. Keep one: remember the new text under the old slug and forget the new slug.\n"
            + "- supersedes: the new memory is a later decision on the same question that names or implies a change (\"instead of\", \"moved to\", \"now\", \"since <date>\", \"no longer\"). Remember the new one again with supersedes: [old slug].\n"
            + "- contradicts: following one would break the other and the new memory does not say it replaces it (e.g. two sources disagree). Record it with settle(\"" + slug + "\", \"conflict\", <old slug>) and ask the user which holds; change nothing else yet.\n"
            + "Decide from the texts, not the dates (updated_at of a memory imported from a file is when that file last changed). When the superseded or contradicting memory has a file source, tell the user that file must change too, or the old rule stays in what the harness loads from there.";
        // An imported memory waits for this check until an agent settles it (conflicts()).
        if imported && g.v(id).in_("supersedes").has_label("memory").filter(visible).count().next() == 0 {
            g.v(id).property("unchecked", true).iterate();
            out.unchecked = true;
        }
    } else if imported {
        g.v(id).properties("unchecked").drop().iterate();
    }
    out
}
