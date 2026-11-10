fn export_memories(project) {
    // Every memory (project "" = of every project and the global ones; else that project's and
    // the global ones), in list order, with what the file memory format keeps.
    let t = g.v().has_label("memory");
    if project != "" { t = t.or(__.not(__.has("project")), __.has("project", project)); }
    t.order().by(__.coalesce(__.values("index_order"), __.constant(1000000))).by("name")
        .project("slug", "project", "kind", "summary", "body", "title", "hook", "links")
        .by("name").by(__.coalesce(__.values("project"), __.constant("")))
        .by("kind").by("summary").by(__.coalesce(__.values("body"), __.constant("")))
        .by(__.coalesce(__.values("title"), __.constant(""))).by(__.coalesce(__.values("hook"), __.constant("")))
        .by(__.out("relates_to").values("name").dedup().fold())
        .to_list()
}
