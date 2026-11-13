fn open_conflicts(project) {
    // The open conflicts between this project's and the global memories (both versions, with
    // whether each is always loaded and its file when it was imported), and how many imported
    // memories wait for their conflict check (project "" = the global ones only), and the same
    // for memories against the lines of the harness's instruction files.
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    let side = __.project("slug", "scope", "kind", "title", "hook", "always", "file")
        .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
        .by("kind").by(__.coalesce(__.values("title"), __.values("name")))
        .by(__.coalesce(__.values("hook"), __.values("summary")))
        .by(__.choose(__.or(__.has("pinned", true), __.has("kind", within(["feedback", "user"]))), __.constant(true), __.constant(false)))
        .by(__.choose(__.has("imported_at"), __.coalesce(__.values("source"), __.constant("")), __.constant("")));
    let open = g.v().has_label("memory").filter(visible).out_e("conflicts_with").filter(__.in_v().filter(visible))
        .project("a", "b").by(__.out_v().map(side)).by(__.in_v().map(side)).to_list();
    let unchecked = g.v().has_label("memory").filter(visible).has("unchecked", true).count().next();
    // Memories against lines of the harness's instruction files: the open conflicts (the
    // memory's side, and the line) and how many candidates wait for their check.
    let current = __.in_v().filter(visible).not(__.in_("supersedes").has_label("memory").filter(visible));
    let rules_open = g.v().has_label("instruction").filter(visible).out_e("compared_with").has("decision", "conflict")
        .filter(current).project("memory", "file", "line", "text")
        .by(__.in_v().map(side)).by(__.out_v().values("file")).by(__.out_v().values("line")).by(__.out_v().values("text")).to_list();
    let rules_unchecked = g.v().has_label("instruction").filter(visible).out_e("compared_with").has("decision", "unchecked")
        .filter(current).count().next();
    #{open: open, unchecked: unchecked, rules_open: rules_open, rules_unchecked: rules_unchecked}
}
