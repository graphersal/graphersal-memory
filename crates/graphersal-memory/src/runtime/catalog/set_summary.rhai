fn set_summary(id, p, facts) {
    if g.v(id).count().next() == 0 { return #{error: "no session " + id}; }
    g.merge_v(#{"T.id": id, "T.label": "session"}).option(Merge.onMatch, p).iterate();
    g.v(id).out("contains").has_label("fact").drop().iterate();
    let e = #{origin: "sessions", source: "derived", visibility: "private"};
    for f in facts {
        g.merge_v(#{"T.id": f.id, "T.label": "fact"}).option(Merge.onCreate, f.p).option(Merge.onMatch, f.p).iterate();
        g.merge_e(#{"T.label": "contains", "Direction.OUT": id, "Direction.IN": f.id}).option(Merge.onCreate, e).iterate();
    }
    #{facts: facts.len()}
}
