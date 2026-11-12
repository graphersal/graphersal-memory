fn overview(project) {
    // The counts of the browser UI's overview: this project's and the global memories (project
    // "" = the global ones only), the open conflicts, the imported memories not yet checked,
    // and the indexed sessions and facts of this project.
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    let rows = g.v().has_label("memory").filter(visible)
        .project("kind", "scoped", "pinned", "superseded", "unchecked", "imported")
        .by("kind").by(__.choose(__.has("project"), __.constant(true), __.constant(false)))
        .by(__.coalesce(__.values("pinned"), __.constant(false)))
        .by(__.in_("supersedes").has_label("memory").filter(visible).count())
        .by(__.coalesce(__.values("unchecked"), __.constant(false)))
        .by(__.choose(__.has("imported_at"), __.constant(true), __.constant(false))).to_list();
    let out = #{memories: rows.len(), project_memories: 0, global_memories: 0, user: 0, feedback: 0,
                project_kind: 0, reference: 0, pinned: 0, superseded: 0, unchecked: 0, imported: 0,
                always_loaded: 0};
    for r in rows {
        if r.scoped { out.project_memories += 1; } else { out.global_memories += 1; }
        if r.kind == "user" { out.user += 1; }
        else if r.kind == "feedback" { out.feedback += 1; }
        else if r.kind == "project" { out.project_kind += 1; }
        else if r.kind == "reference" { out.reference += 1; }
        if r.pinned == true { out.pinned += 1; }
        if r.superseded > 0 { out.superseded += 1; }
        if r.unchecked == true { out.unchecked += 1; }
        if r.imported { out.imported += 1; }
        if r.superseded == 0 && (r.pinned == true || r.kind == "feedback" || r.kind == "user") { out.always_loaded += 1; }
    }
    out.open_conflicts = g.v().has_label("memory").filter(visible).out_e("conflicts_with")
        .filter(__.in_v().has_label("memory").filter(visible)).count().next();
    out.sessions = g.v().has_label("session").filter(visible).count().next();
    out.facts = g.v().has_label("fact").filter(visible).count().next();
    out
}
