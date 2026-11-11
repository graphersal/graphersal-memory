fn forget(slug, project) {
    // The current project's memory of that slug first, then the global one.
    let id = "";
    if project != "" && g.v("memory:" + slug + "@" + project).has_label("memory").count().next() > 0 { id = "memory:" + slug + "@" + project; }
    else if g.v("memory:" + slug).has_label("memory").count().next() > 0 { id = "memory:" + slug; }
    if id == "" { return #{slug: slug, removed: false}; }
    let scope = if id.contains("@") { "project" } else { "global" };
    let file = g.v(id).has("imported_at").values("source").to_list();
    g.v(id).has_label("memory").drop().iterate();
    let out = #{slug: slug, scope: scope, removed: true};
    // An unchanged file never brings it back; its text is still in the file the harness reads.
    if file.len() > 0 {
        out.file = file[0];
        out.note = "It was imported from this file: tell the user to edit or delete it too, or its old text stays in what the harness loads from there.";
    }
    out
}
