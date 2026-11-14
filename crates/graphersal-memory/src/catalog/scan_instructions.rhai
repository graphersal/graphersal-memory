fn scan_instructions(project, limit) {
    // The conflict check's candidates for up to `limit` rules not yet scanned (only directive
    // lines are rules: crate::instructions): the nearest memories of each (the recall scoring on
    // the rule's text; a project's rule against the project's and the global memories, a global
    // rule against the global ones), at most 3 per rule, each with a semantic similarity of at
    // least 0.6 AND at least 2 content words shared with the rule (words of 3+ letters, stop
    // words and directive markers left out, the first 6 letters compared), superseded memories
    // left out. The bar is high on purpose: a candidate only on the same topic is noise for the
    // agent. They become `compared_with` edges with decision "unchecked" for the agent to settle.
    // No new candidates while 20 wait for their check (the rest of the rules stay unscanned
    // until then). remember.rhai applies the same bar to a new memory. Only with an embedding
    // model (the schema's meta.instruction_check, crate::catalog::instruction_check): without
    // one nothing is scanned (off: true) and the lines wait for a model.
    let cap = 20;
    let stop = "the and for with from into this that these those are was were been has have had not but all any can cannot use used uses using when what which who why how where its our your you they them their then than there here also only never always must should don don't does did each every other anothe more most some such very just like about over under after before withou within will would could may might one two new via per etc instea prefer avoid requir make sure user agent it's".split(" ");
    let mine = if project == "" { __.not(__.has("project")) }
               else { __.or(__.not(__.has("project")), __.has("project", project)) };
    let schema = g.get_schema();
    let check = type_of(schema) == "map" && type_of(schema.meta) == "map" && schema.meta.instruction_check == true;
    if !check {
        let remaining = g.v().has_label("instruction").filter(mine).has("scanned", false).count().next();
        return #{scanned: 0, found: 0, remaining: remaining, waiting: 0, capped: false, off: true};
    }
    let now = g.gql("RETURN CURRENT_TIMESTAMP AS t").next()["t"];
    let waiting = g.v().has_label("instruction").filter(mine).out_e("compared_with").has("decision", "unchecked")
        .filter(__.in_v().filter(mine)).count().next();
    let todo = g.v().has_label("instruction").filter(mine).has("scanned", false)
        .order().by("file").by("line").limit(limit)
        .project("id", "text", "project").by(T.id).by("text")
        .by(__.coalesce(__.values("project"), __.constant(""))).to_list();
    let scanned = 0;
    let found = 0;
    for r in todo {
        if waiting >= cap { break; }
        let visible = if r.project == "" { __.not(__.has("project")) }
                      else { __.or(__.not(__.has("project")), __.has("project", r.project)) };
        // The rule's content words (their first 6 letters).
        let words = [];
        let cur = "";
        for c in (r.text.to_lower() + " ").chars() {
            if (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '\'' { cur += c; continue; }
            if cur.len() > 6 { cur = cur.sub_string(0, 6); }
            if cur.len() >= 3 && !stop.contains(cur) && !words.contains(cur) { words.push(cur); }
            cur = "";
        }
        let hits = g.query("recall", #{text: r.text, k: 12, label: "memory", project: r.project});
        let taken = 0;
        for h in hits {
            if taken >= 3 { break; }
            if h.semantic < 0.6 { continue; }
            // At least 2 content words shared with the memory's text.
            let theirs = [];
            let cur = "";
            let text = "";
            for x in g.v(h.id).values("embed_text", "body").to_list() { text += " " + x; }
            for c in (text.to_lower() + " ").chars() {
                if (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '\'' { cur += c; continue; }
                if cur.len() > 6 { cur = cur.sub_string(0, 6); }
                if cur.len() >= 3 && !theirs.contains(cur) { theirs.push(cur); }
                cur = "";
            }
            let common = 0;
            for w in words { if theirs.contains(w) { common += 1; } }
            if common < 2 { continue; }
            if g.v(h.id).in_("supersedes").has_label("memory").filter(visible).count().next() > 0 { continue; }
            taken += 1;
            if g.v(r.id).out_e("compared_with").filter(__.in_v().has_id(h.id)).count().next() > 0 { continue; }
            g.merge_e(#{"T.label": "compared_with", "Direction.OUT": r.id, "Direction.IN": h.id})
                .option(Merge.onCreate, #{origin: "instructions", source: "derived", visibility: "private", decision: "unchecked", at: now}).iterate();
            waiting += 1;
            found += 1;
        }
        g.v(r.id).property("scanned", true).iterate();
        scanned += 1;
    }
    let remaining = g.v().has_label("instruction").filter(mine).has("scanned", false).count().next();
    #{scanned: scanned, found: found, remaining: remaining, waiting: waiting, capped: waiting >= cap && remaining > 0}
}
