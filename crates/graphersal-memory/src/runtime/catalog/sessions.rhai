fn sessions(text, k, project) {
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    if text == "" {
        return g.v().has_label("session").filter(visible).order().by("started_at", Order.desc).limit(k)
            .project("id", "started_at", "title", "headline", "summary", "branches")
            .by(T.id).by("started_at").by(__.coalesce(__.values("title"), __.constant("")))
            .by("name").by(__.coalesce(__.values("summary"), __.constant("")))
            .by(__.coalesce(__.values("branches"), __.constant([]))).to_list();
    }
    let hits = g.query("recall", #{text: text, k: k, label: "fact", project: project});
    let out = [];
    for h in hits {
        out.push(g.v(h.id).project("fact", "kind", "why", "at", "session", "headline", "score")
            .by("summary").by("fact_kind").by(__.coalesce(__.values("why"), __.constant("")))
            .by(__.coalesce(__.values("at"), __.constant(""))).by(__.coalesce(__.in_("contains").id(), __.constant("")))
            .by(__.coalesce(__.in_("contains").values("name"), __.constant(""))).by(__.constant(h.score)).next());
    }
    out
}
