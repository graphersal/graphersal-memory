fn recall(text, k, label, project) {
    // Only what the current project may see: global vertices (no project) and its own.
    let visible = if project == "" { __.not(__.has("project")) }
                  else { __.or(__.not(__.has("project")), __.has("project", project)) };
    let words = [];
    for w in text.to_lower().split(" ") {
        w.trim();
        if w.len() > 2 && !["how", "add", "the", "and", "for", "with", "does", "what", "where", "when", "from", "into", "use", "can", "should", "which", "who", "why", "new", "via"].contains(w) { words.push(if w.len() > 6 { w.sub_string(0, 6) } else { w }); }
    }
    // The lines of instruction files are there only for the conflict check: the harness loads
    // them itself, so a search for everything leaves them out.
    let t = if label == "" { g.v().has("emb").not(__.has_label("instruction")) } else { g.v().has_label(label).has("emb") };
    let rows = t.filter(visible)
        .project("id", "label", "name", "summary", "semantic", "hay")
        .by(T.id).by(T.label).by(__.coalesce(__.values("name"), __.constant("")))
        .by(__.coalesce(__.values("summary"), __.constant("")))
        .by(__.similarity("emb", text))
        .by(__.values("embed_text", "body", "user").fold())
        .to_list();
    // keyword weight of a word = its IDF over the candidates, so common words count little
    let df = [];
    for w in words { df.push(0); }
    for i in 0..rows.len() {
        let hay = "";
        for x in rows[i].hay { hay += " " + x; }
        hay = hay.to_lower();
        rows[i].hay = hay;
        for j in 0..words.len() { if hay.contains(words[j]) { df[j] += 1; } }
    }
    let n = rows.len().to_float() + 1.0;
    let idf = [];
    let idf_total = 0.0;
    for j in 0..words.len() { let x = (n / (df[j].to_float() + 1.0)).ln(); idf.push(x); idf_total += x; }
    for i in 0..rows.len() {
        let hit = 0.0;
        for j in 0..words.len() { if rows[i].hay.contains(words[j]) { hit += idf[j]; } }
        let lexical = if idf_total <= 0.0 { 0.0 } else { hit / idf_total };
        rows[i].keywords = lexical;
        rows[i].score = rows[i].semantic + 0.35 * lexical;
        rows[i].remove("hay");
    }
    let top = [];
    let taken = [];
    for n in 0..k {
        let best = -1;
        for i in 0..rows.len() {
            if !taken.contains(i) && (best < 0 || rows[i].score > rows[best].score) { best = i; }
        }
        if best < 0 { break; }
        taken.push(best);
        let row = rows[best];
        // A superseded memory is marked (it leads to its successor), an open conflict named.
        if row.label == "memory" {
            let sup = g.v(row.id).in_e("supersedes").filter(__.out_v().has_label("memory").filter(visible))
                .project("slug", "at").by(__.out_v().values("name"))
                .by(__.coalesce(__.values("at").as_string(), __.out_v().values("updated_at").as_string(), __.constant(""))).to_list();
            if sup.len() > 0 {
                let at = if sup[0].at.len() >= 10 { sup[0].at.sub_string(0, 10) } else { sup[0].at };
                row.summary = "SUPERSEDED by " + sup[0].slug + " on " + at + ": " + row.summary;
                row.superseded_by = sup[0].slug;
            }
            let c = g.v(row.id).both("conflicts_with").has_label("memory").filter(visible).values("name").to_list();
            if c.len() > 0 { row.open_conflict_with = c; }
        }
        top.push(row);
    }
    top
}
