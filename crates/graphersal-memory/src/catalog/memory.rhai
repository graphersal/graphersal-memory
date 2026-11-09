fn memory(slug, project) {
    let s = if slug.starts_with("memory:") { slug.sub_string(7) } else { slug };
    // The current project's memory of that slug first, then the global one.
    let id = "";
    if project != "" && g.v("memory:" + s + "@" + project).has_label("memory").count().next() > 0 { id = "memory:" + s + "@" + project; }
    else if g.v("memory:" + s).has_label("memory").count().next() > 0 { id = "memory:" + s; }
    if id == "" { return #{error: "no memory " + s + " in this project or the global memory; list them with memories()"}; }
    let m = g.v(id).project("slug", "scope", "kind", "title", "hook", "summary", "body", "updated_at")
        .by("name").by(__.coalesce(__.values("project").constant("project"), __.constant("global")))
        .by("kind").by(__.coalesce(__.values("title"), __.values("name")))
        .by(__.coalesce(__.values("hook"), __.values("summary"))).by("summary")
        .by(__.coalesce(__.values("body"), __.constant(""))).by("updated_at").next();
    m.links = g.v(id).out("relates_to", "supersedes").has_label("memory").values("name").to_list();
    m.linked_from = g.v(id).in_("relates_to", "supersedes").has_label("memory").values("name").to_list();
    m.about = g.v(id).out("about").id().to_list();
    m
}
