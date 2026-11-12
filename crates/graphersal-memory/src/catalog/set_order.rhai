fn set_order(slugs, project) {
    // The browser UI's move up / down: the memories named, in this order, get the priorities
    // 10, 20, 30, ... (one unit: a slug that is not a memory of this project or a global one
    // changes nothing).
    let ids = [];
    for s in slugs {
        let id = "";
        if project != "" && g.v("memory:" + s + "@" + project).has_label("memory").count().next() > 0 { id = "memory:" + s + "@" + project; }
        else if g.v("memory:" + s).has_label("memory").count().next() > 0 { id = "memory:" + s; }
        if id == "" { return #{error: "no memory " + s + " in this project or the global memory; nothing was reordered"}; }
        if ids.contains(id) { return #{error: "the memory " + s + " is named twice; nothing was reordered"}; }
        ids.push(id);
    }
    let orders = [];
    for i in 0..ids.len() {
        let o = (i + 1) * 10;
        g.v(ids[i]).property("order", o).iterate();
        orders.push(#{slug: slugs[i], order: o});
    }
    #{reordered: orders.len(), orders: orders}
}
