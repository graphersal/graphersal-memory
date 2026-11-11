fn migrate_import_sources(records) {
    // One-time (graphersal-memory M16): memories imported before the import marked them (source
    // "user", no imported_at) get the file they came from and the time of their import, from
    // the import's records ({path, slug, project, imported_at}).
    let migrated = 0;
    for r in records {
        if type_of(r.slug) != "string" || r.slug == "" || type_of(r.path) != "string" { continue; }
        let id = if type_of(r.project) == "string" && r.project != "" { "memory:" + r.slug + "@" + r.project } else { "memory:" + r.slug };
        if g.v(id).has_label("memory").has("source", "user").count().next() == 0 { continue; }
        if g.v(id).has("imported_at").count().next() > 0 { continue; }
        g.v(id).property("source", r.path).iterate();
        if type_of(r.imported_at) == "string" && r.imported_at != "" {
            g.v(id).property("imported_at", r.imported_at).iterate();
        } else {
            g.v(id).property("imported_at", g.gql("RETURN CURRENT_TIMESTAMP AS t").next()["t"]).iterate();
        }
        migrated += 1;
    }
    #{migrated: migrated}
}
