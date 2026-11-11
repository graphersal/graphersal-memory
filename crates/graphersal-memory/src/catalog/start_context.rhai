fn start_context(project) {
    // The memories loaded into the start of every session: every pinned one and every feedback
    // and user memory of this project and the global ones (project "" = the global ones only).
    // The product orders, groups and cuts them (crate::start_context).
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    g.v().has_label("memory").filter(visible)
        .or(__.has("pinned", true), __.has("kind", within(["feedback", "user"])))
        .project("slug", "scope", "kind", "pinned", "order", "title", "hook", "updated_at")
        .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
        .by("kind").by(__.coalesce(__.values("pinned"), __.constant(false)))
        .by(__.values("order").fold())
        .by(__.coalesce(__.values("title"), __.values("name")))
        .by(__.coalesce(__.values("hook"), __.values("summary")))
        .by(__.coalesce(__.values("updated_at").as_string(), __.constant("")))
        .to_list()
}
