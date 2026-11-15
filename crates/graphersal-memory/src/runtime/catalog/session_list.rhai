fn session_list(project, before, limit) {
    // The browser UI's sessions by day: this project's sessions that started before `before`
    // (an ISO time; "" = now), newest first, with how many facts of each kind their summary
    // holds. The next page starts before the last row's started_at.
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    let t = g.v().has_label("session").filter(visible);
    if before != "" { t = t.filter(__.values("started_at").as_string().is(P.lt(before))); }
    let rows = t.order().by(__.values("started_at").as_string(), Order.desc).limit(limit)
        .project("id", "started_at", "ended_at", "headline", "title", "harness", "turns", "kinds")
        .by(T.id).by(__.values("started_at").as_string())
        .by(__.coalesce(__.values("ended_at").as_string(), __.constant("")))
        .by(__.coalesce(__.values("name"), __.constant(""))).by(__.coalesce(__.values("title"), __.constant("")))
        .by(__.coalesce(__.values("harness"), __.constant(""))).by(__.coalesce(__.values("turns"), __.constant(0)))
        .by(__.out("contains").has_label("fact").values("fact_kind").fold()).to_list();
    for i in 0..rows.len() {
        let c = #{decision: 0, correction: 0, outcome: 0, open_thread: 0};
        for k in rows[i].kinds { if c.contains(k) { c[k] += 1; } }
        rows[i].decisions = c.decision;
        rows[i].corrections = c.correction;
        rows[i].outcomes = c.outcome;
        rows[i].open_threads = c.open_thread;
        rows[i].remove("kinds");
    }
    rows
}
