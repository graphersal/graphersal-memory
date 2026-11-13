fn conflicts(k, project) {
    // What waits for a decision in this project's and the global memories: open conflicts
    // (both versions), imported memories not yet checked against their nearest memories, the
    // project memories that narrow a global one, and memories against the lines of the
    // harness's instruction files near them (open conflicts and candidates still to check).
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    let open = [];
    let pairs = g.v().has_label("memory").filter(visible).out_e("conflicts_with").filter(__.in_v().filter(visible))
        .project("a", "b", "since").by(__.out_v().id()).by(__.in_v().id())
        .by(__.coalesce(__.values("at").as_string(), __.constant(""))).to_list();
    for pair in pairs {
        let row = #{since: if pair.since.len() >= 10 { pair.since.sub_string(0, 10) } else { pair.since }};
        let files = [];
        for key in ["a", "b"] {
            let m = g.v(pair[key]).project("slug", "scope", "kind", "summary", "source", "updated_at")
                .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
                .by("kind").by("summary")
                .by(__.choose(__.has("imported_at"), __.coalesce(__.values("source"), __.constant("remember")), __.choose(__.has("source", "ui"), __.constant("ui"), __.constant("remember"))))
                .by(__.coalesce(__.values("updated_at").as_string(), __.constant(""))).next();
            if m.source != "remember" && m.source != "ui" { files.push(m.source); }
            row[key] = m;
        }
        if files.len() > 0 { row.files = files; }
        open.push(row);
    }
    let all = g.v().has_label("memory").filter(visible).has("unchecked", true)
        .order().by(__.coalesce(__.values("index_order"), __.constant(1000000))).by("name").id().to_list();
    let unchecked = [];
    for id in all {
        if unchecked.len() >= k { break; }
        let m = g.v(id).project("slug", "scope", "kind", "summary", "source", "updated_at")
            .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
            .by("kind").by("summary").by(__.coalesce(__.values("source"), __.constant("remember")))
            .by(__.coalesce(__.values("updated_at").as_string(), __.constant(""))).next();
        let hits = g.query("recall", #{text: m.slug + ": " + m.summary, k: 12, label: "memory", project: project});
        let nearest = [];
        for h in hits {
            if nearest.len() >= 5 { break; }
            if h.id == id { continue; }
            if g.v(h.id).in_("supersedes").has_label("memory").filter(visible).count().next() > 0 { continue; }
            nearest.push(g.v(h.id).project("slug", "scope", "kind", "summary", "source", "updated_at")
                .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
                .by("kind").by("summary")
                .by(__.choose(__.has("imported_at"), __.coalesce(__.values("source"), __.constant("remember")), __.choose(__.has("source", "ui"), __.constant("ui"), __.constant("remember"))))
                .by(__.coalesce(__.values("updated_at").as_string(), __.constant(""))).next());
        }
        m.nearest = nearest;
        unchecked.push(m);
    }
    let narrows = g.v().has_label("memory").filter(visible).out_e("narrows").filter(__.in_v().filter(visible))
        .project("slug", "global").by(__.out_v().values("name")).by(__.in_v().values("name")).to_list();
    for i in 0..narrows.len() { narrows[i].note = "narrows the global rule " + narrows[i].global; }
    // Imported memories that were superseded: their files still hold the old rule.
    let stale = g.v().has_label("memory").filter(visible).has("imported_at")
        .filter(__.in_("supersedes").has_label("memory").filter(visible))
        .project("slug", "file", "superseded_by").by("name").by("source")
        .by(__.in_("supersedes").has_label("memory").filter(visible).values("name").limit(1)).to_list();
    // Memories against lines of instruction files (compared_with: instruction -> memory), the
    // superseded memories left out.
    let lines = g.v().has_label("instruction").filter(visible).out_e("compared_with")
        .filter(__.in_v().filter(visible).not(__.in_("supersedes").has_label("memory").filter(visible)))
        .project("rule", "memory", "decision", "since", "file", "line")
        .by(__.out_v().id()).by(__.in_v().id()).by(__.coalesce(__.values("decision"), __.constant("unchecked")))
        .by(__.coalesce(__.values("at").as_string(), __.constant("")))
        .by(__.out_v().values("file")).by(__.out_v().values("line")).to_list();
    let rules_open = [];
    let rules_unchecked = [];
    let rules_total = 0;
    for pair in lines {
        if pair.decision == "compatible" { continue; }
        if pair.decision != "conflict" {
            rules_total += 1;
            if rules_unchecked.len() >= k { continue; }
        }
        let row = #{since: if pair.since.len() >= 10 { pair.since.sub_string(0, 10) } else { pair.since }};
        row.memory = g.v(pair.memory).project("slug", "scope", "kind", "summary", "source", "updated_at")
            .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
            .by("kind").by("summary")
            .by(__.choose(__.has("imported_at"), __.coalesce(__.values("source"), __.constant("remember")), __.choose(__.has("source", "ui"), __.constant("ui"), __.constant("remember"))))
            .by(__.coalesce(__.values("updated_at").as_string(), __.constant(""))).next();
        row.rule = g.v(pair.rule).project("file", "line", "text", "scope", "loaded_by")
            .by("file").by("line").by("text")
            .by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
            .by(__.coalesce(__.values("loaded_by"), __.constant(""))).next();
        row.rule.rule = pair.rule.sub_string(12);
        if pair.decision == "conflict" { rules_open.push(row); } else { rules_unchecked.push(row); }
    }
    let out = #{open: open, unchecked: unchecked, unchecked_total: all.len()};
    if rules_open.len() > 0 {
        out.rules_open = rules_open;
        out.settle_rules = "Each memory contradicts a line of an instruction file the harness loads. Ask the user which holds. The memory is outdated: forget it, or remember the new rule with supersedes: [<slug>] (an imported memory: the user edits or deletes its file too). The file is outdated: tell the user to edit that file at that line (never edit it yourself); the conflict closes when the line changes or goes. Both hold after all: settle(<slug>, \"compatible\", rule: <rule>).";
    }
    if rules_total > 0 {
        out.rules_unchecked = rules_unchecked;
        out.rules_unchecked_total = rules_total;
        out.check_rules = "Each pair is a memory and a line of an instruction file the harness loads near it; most are merely on a similar topic. Per pair pick ONE: compatible (both can hold: different subject, scope, tool or an explicit exception; a project's rule that narrows a global memory is compatible; the default): settle(<slug>, \"compatible\", rule: <rule>); conflict (following one would break the other): settle(<slug>, \"conflict\", rule: <rule>) and ask the user which holds. Decide from the texts. Never edit an instruction file yourself.";
    }
    if narrows.len() > 0 { out.narrows = narrows; }
    if stale.len() > 0 {
        out.stale_files = stale;
        out.edit_files = "These files still hold a superseded rule: tell the user to edit or delete them, or the old rule stays in what the harness loads from there.";
    }
    if open.len() > 0 {
        out.settle_open = "For each open conflict ask the user which memory holds, then settle(<winner>, \"supersedes\", <loser>), or forget the loser; when both hold, settle(<a>, \"compatible\", <b>). When the losing memory has a file source, tell the user to edit or delete that file too, or its old rule stays in what the harness loads from there.";
    }
    if unchecked.len() > 0 {
        out.check = "Each unchecked memory was imported from a file and not yet checked against its nearest memories; most are merely on a similar topic. Per nearest memory pick ONE: compatible (both can hold: different subject, scope, folder, tool or an explicit exception; the default; a project memory that narrows a global one is compatible: settle(<project slug>, \"narrows\", <global slug>)), replaces (the same rule in other words: keep one, forget the other), supersedes (a later decision on the same question that names or implies a change: settle(<newer>, \"supersedes\", <older>)), contradicts (following one would break the other, within one scope: settle(<slug>, \"conflict\", <other>) and ask the user which holds). Decide from the texts, not the dates. Then settle(<slug>, \"compatible\") ends the check of a memory whose nearest are all compatible.";
    }
    out
}
