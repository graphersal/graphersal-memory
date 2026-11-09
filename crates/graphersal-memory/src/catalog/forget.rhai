fn forget(slug, project) {
    // The current project's memory of that slug first, then the global one.
    let id = "";
    if project != "" && g.v("memory:" + slug + "@" + project).has_label("memory").count().next() > 0 { id = "memory:" + slug + "@" + project; }
    else if g.v("memory:" + slug).has_label("memory").count().next() > 0 { id = "memory:" + slug; }
    if id == "" { return #{slug: slug, removed: false}; }
    let scope = if id.contains("@") { "project" } else { "global" };
    g.v(id).has_label("memory").drop().iterate();
    #{slug: slug, scope: scope, removed: true}
}
