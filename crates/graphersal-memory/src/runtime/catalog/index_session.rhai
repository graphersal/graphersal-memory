fn index_session(id, p) {
    // Not complete until the last batch of turns sets the fingerprint again.
    p.fingerprint = "";
    // A summary stays until a new one replaces it.
    if g.v(id).has("summarized").count().next() > 0 {
        p.remove("name");
        p.remove("summary");
        p.remove("embed_text");
    }
    g.merge_v(#{"T.id": id, "T.label": "session"}).option(Merge.onCreate, p).option(Merge.onMatch, p).iterate();
    g.v(id).out("contains").has_label("turn").drop().iterate();
    #{id: id}
}
