fn index_turns(id, rows, fingerprint) {
    let e = #{origin: "sessions", source: "derived", visibility: "private"};
    for r in rows {
        g.merge_v(#{"T.id": r.id, "T.label": "turn"}).option(Merge.onCreate, r.p).option(Merge.onMatch, r.p).iterate();
        g.merge_e(#{"T.label": "contains", "Direction.OUT": id, "Direction.IN": r.id}).option(Merge.onCreate, e).iterate();
    }
    if fingerprint != "" {
        let done = #{fingerprint: fingerprint};
        g.merge_v(#{"T.id": id, "T.label": "session"}).option(Merge.onMatch, done).iterate();
    }
    #{written: rows.len()}
}
