fn scan_instructions(project, limit) {
    // The conflict check's candidates for up to `limit` rules not yet scanned: the nearest
    // memories of each (the recall scoring on the rule's text; a project's rule against the
    // project's and the global memories, a global rule against the global ones), at most 3 per
    // rule, each scoring at least 0.15 and 1.5 times the 12th best score and sharing a subject
    // word with the rule (or a semantic similarity of 0.45), superseded memories left out. They
    // become `compared_with` edges with decision "unchecked" for the agent or the user to settle.
    // No new candidates while 50 wait for their check (the rest of the rules stay unscanned until
    // then).
    let mine = if project == "" { __.not(__.has("project")) }
               else { __.or(__.not(__.has("project")), __.has("project", project)) };
    let now = g.gql("RETURN CURRENT_TIMESTAMP AS t").next()["t"];
    let waiting = g.v().has_label("instruction").filter(mine).out_e("compared_with").has("decision", "unchecked")
        .filter(__.in_v().filter(mine)).count().next();
    let todo = g.v().has_label("instruction").filter(mine).has("scanned", false)
        .order().by("file").by("line").limit(limit)
        .project("id", "text", "project").by(T.id).by("text")
        .by(__.coalesce(__.values("project"), __.constant(""))).to_list();
    let scanned = 0;
    let found = 0;
    for r in todo {
        if waiting >= 50 { break; }
        let visible = if r.project == "" { __.not(__.has("project")) }
                      else { __.or(__.not(__.has("project")), __.has("project", r.project)) };
        let hits = g.query("recall", #{text: r.text, k: 12, label: "memory", project: r.project});
        let floor = 0.15;
        if hits.len() >= 12 && hits[11].score * 1.5 > floor { floor = hits[11].score * 1.5; }
        let taken = 0;
        for h in hits {
            if taken >= 3 || h.score < floor { break; }
            // A shared subject word, or a meaning clearly alike (a model embedder).
            if h.keywords <= 0.0 && h.semantic < 0.45 { continue; }
            if g.v(h.id).in_("supersedes").has_label("memory").filter(visible).count().next() > 0 { continue; }
            taken += 1;
            if g.v(r.id).out_e("compared_with").filter(__.in_v().has_id(h.id)).count().next() > 0 { continue; }
            g.merge_e(#{"T.label": "compared_with", "Direction.OUT": r.id, "Direction.IN": h.id})
                .option(Merge.onCreate, #{origin: "instructions", source: "derived", visibility: "private", decision: "unchecked", at: now}).iterate();
            waiting += 1;
            found += 1;
        }
        g.v(r.id).property("scanned", true).iterate();
        scanned += 1;
    }
    let remaining = g.v().has_label("instruction").filter(mine).has("scanned", false).count().next();
    #{scanned: scanned, found: found, remaining: remaining, waiting: waiting, capped: waiting >= 50 && remaining > 0}
}
