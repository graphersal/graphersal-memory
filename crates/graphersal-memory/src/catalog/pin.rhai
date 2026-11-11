fn pin(slug, pinned, order, project) {
    // The current project's memory of that slug first, then the global one.
    let id = "";
    if project != "" && g.v("memory:" + slug + "@" + project).has_label("memory").count().next() > 0 { id = "memory:" + slug + "@" + project; }
    else if g.v("memory:" + slug).has_label("memory").count().next() > 0 { id = "memory:" + slug; }
    if id == "" { return #{error: "no memory " + slug + " in this project or the global memory; list them with memories()"}; }
    g.v(id).property("pinned", pinned).iterate();
    if type_of(order) == "i64" { g.v(id).property("order", order).iterate(); }
    let o = g.v(id).values("order").to_list();
    #{slug: slug, scope: if id.contains("@") { "project" } else { "global" }, pinned: pinned,
      order: if o.len() == 0 { () } else { o[0] }}
}
