fn instruction_backlog() {
    // The projects whose instruction lines wait for their scan ("" for the lines of user-level
    // files): the daemon queues their scan once the store has an embedding model (lines synced
    // while the check was off, crate::catalog::instruction_check).
    let projects = g.v().has_label("instruction").has("scanned", false).has("project")
        .values("project").dedup().to_list();
    if g.v().has_label("instruction").has("scanned", false).not(__.has("project")).count().next() > 0 {
        projects.push("");
    }
    projects
}
