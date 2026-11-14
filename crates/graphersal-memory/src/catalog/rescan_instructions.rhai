fn rescan_instructions() {
    // One-time (graphersal-memory 2026-11-14: only directive lines are rules, a higher bar for
    // candidates): the candidates still waiting for their check are dropped and every
    // instruction line of every project is scanned again (scan_instructions); the settled
    // decisions (compatible, conflict) stay.
    let dropped = g.e().has_label("compared_with").has("decision", "unchecked").count().next();
    g.e().has_label("compared_with").has("decision", "unchecked").drop().iterate();
    let lines = g.v().has_label("instruction").count().next();
    g.v().has_label("instruction").property("scanned", false).iterate();
    #{dropped: dropped, lines: lines}
}
