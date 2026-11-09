fn indexed_sessions() {
    g.v().has_label("session").project("id", "fingerprint", "summarized")
        .by(T.id).by(__.coalesce(__.values("fingerprint"), __.constant("")))
        .by(__.coalesce(__.values("summarized"), __.constant(""))).to_list()
}
