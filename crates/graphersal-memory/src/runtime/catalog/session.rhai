fn session(id, turns, project) {
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    let sid = if id.starts_with("session:") { id } else { "session:" + id };
    if g.v(sid).has_label("session").filter(visible).count().next() == 0 {
        let m = g.v().has_label("session").filter(visible).id().filter(__.is(P.starting_with(sid))).limit(2).to_list();
        if m.len() != 1 { return #{error: "no unique session of this project matches " + id, candidates: m}; }
        sid = m[0];
    }
    let s = g.v(sid).project("title", "headline", "body", "started_at", "ended_at", "branches", "files_edited", "turns")
        .by(__.coalesce(__.values("title"), __.constant(""))).by("name")
        .by(__.coalesce(__.values("body"), __.values("summary"), __.constant(""))).by("started_at")
        .by(__.coalesce(__.values("ended_at"), __.constant(""))).by(__.coalesce(__.values("branches"), __.constant([])))
        .by(__.coalesce(__.values("files_edited"), __.constant([]))).by(__.coalesce(__.values("turns"), __.constant(0))).next();
    s.id = sid;
    s.about = g.v(sid).out("about").id().to_list();
    if turns {
        let tl = [];
        for t in g.v(sid).out("contains").has_label("turn").order().by("n").project("n", "at", "user").by("n").by("at").by(__.coalesce(__.values("user"), __.constant(""))).to_list() {
            let u = t.user;
            if u.len() > 120 { u = u.sub_string(0, 120) + "..."; }
            tl.push(#{n: t.n, at: t.at, user: u});
        }
        s.turn_list = tl;
    }
    s
}
