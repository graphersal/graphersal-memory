// The Graph tab's pure logic (no DOM, no Vue): the graph data of the engine's graph view
// (`api.graph`: {nodes: [{id, label, caption, data, classes}], edges: [{id, source, target,
// label, data}]}) from the UI's functions `memory_graph` (the memories and their decision
// edges) and `memory_neighbours` (the sessions and facts around one memory), the style classes
// of a memory (the profile's `graph.classes`: colour by kind, a thick border when it is loaded
// at every session start, a dashed one when it was imported, faded when it was replaced), the
// card rows of the edges and the buttons of the cards with the writes they make. Tested with
// `node --test` (ui/tests/graph-model.test.mjs).
//
// The graph itself never writes: the buttons call the memory functions the other tabs call
// (pin, settle, forget) with the same rules (an imported memory is never forgotten here).

import * as model from './model.js';

/** The class of each memory kind (profile.json `graph.classes`). */
export const KIND_CLASSES = Object.freeze({
  user: 'kind-user', feedback: 'kind-feedback', project: 'kind-project', reference: 'kind-reference',
});

/** The edge labels of the memory graph, as the card titles name them. */
const EDGE_WORDS = Object.freeze({
  supersedes: 'replaces',
  conflicts_with: 'contradicts',
  narrows: 'narrows',
  relates_to: 'relates to',
  about: 'is about',
  contains: 'contains',
  edited: 'edited',
  wrote: 'wrote',
});

/**
 * The style classes of a memory row (`model.memoryRow`): its kind, then `imported` (dashed
 * border), `always` (thick border: loaded at every session start: pinned, feedback, user, not
 * replaced), `superseded` (faded). The order matters: a later class wins a key, the border is
 * merged key by key, so an imported memory that is always loaded gets a thick dashed border.
 */
export function memoryClasses(row) {
  const out = [];
  if (KIND_CLASSES[row.kind]) out.push(KIND_CLASSES[row.kind]);
  if (row.imported) out.push('imported');
  if (row.always) out.push('always');
  if (row.superseded_by) out.push('superseded');
  return out;
}

const slugOfId = (id) => String(id || '').replace(/^memory:/, '').replace(/@.*$/, '');

/**
 * The graph data of `memory_graph` answers for a selection (`forSelection(..., {single: true})`:
 * `[{call, rows: {nodes, edges}}]`; "All projects" = one per project plus the global memories).
 * A memory in several answers (a global one) is one node; it counts as replaced or in conflict
 * when any answer says so. Node `data` is the memory card's row (`model.memoryRow`: where, source,
 * imported, ...); the caption is the slug.
 * @returns {{nodes: Array, edges: Array}}
 */
export function memoryGraph(answers, projects = [], selection = model.ALL) {
  const rows = new Map();
  const order = [];
  const conflicts = new Map();
  for (const { call, rows: answer } of answers || []) {
    for (const raw of (answer && answer.nodes) || []) {
      if (!raw || !raw.id) continue;
      const id = String(raw.id);
      if (!rows.has(id)) {
        rows.set(id, { ...raw, project: raw.project !== undefined ? raw.project : (raw.scope === 'global' ? '' : call.project) });
        order.push(id);
        conflicts.set(id, new Set());
      } else if (raw.superseded_by && !rows.get(id).superseded_by) {
        rows.get(id).superseded_by = raw.superseded_by;
        rows.get(id).always = false;
      }
      for (const c of raw.conflicts || []) conflicts.get(id).add(c);
    }
  }
  const nodes = order.map((id) => {
    const raw = { ...rows.get(id), conflicts: [...conflicts.get(id)] };
    const row = { ...model.memoryRow(raw, projects, selection), label: 'memory', id };
    return { id, label: 'memory', caption: row.slug, data: row, classes: memoryClasses(row) };
  });
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const edges = [];
  const seen = new Set();
  for (const { rows: answer } of answers || []) {
    for (const raw of (answer && answer.edges) || []) {
      const id = String(raw.id);
      if (seen.has(id) || !byId.has(String(raw.source)) || !byId.has(String(raw.target))) continue;
      seen.add(id);
      edges.push({ id, source: String(raw.source), target: String(raw.target), label: raw.label, data: edgeRow(raw, byId) });
    }
  }
  return { nodes, edges };
}

/**
 * The card row of an edge: a title in words ("use-pnpm replaces use-npm"), its ends, when it
 * was recorded, the project its writes pass (the source memory's) and where it holds.
 */
export function edgeRow(raw, byId) {
  const from = byId.get(String(raw.source));
  const to = byId.get(String(raw.target));
  const name = (node, id) => (node ? (node.data.slug || node.caption || node.id) : slugOfId(id));
  const a = name(from, raw.source);
  const b = name(to, raw.target);
  const word = EDGE_WORDS[raw.label] || raw.label;
  const row = {
    label: raw.label,
    title: raw.label === 'narrows' ? `${a} narrows the global ${b}` : `${a} ${word} ${b}`,
    from: a,
    to: b,
    from_scope: from ? from.data.scope : '',
    to_scope: to ? to.data.scope : '',
    at: raw.at || '',
    context: from && from.data.context !== undefined ? from.data.context : '',
    where: from && from.data.where ? from.data.where : '',
  };
  return row;
}

/**
 * The calls of `memory_neighbours` for a memory row: its own project for a project memory; for
 * a global one the selection's calls (every project and the global memories when "All
 * projects": sessions and facts belong to a project).
 */
export function neighbourCalls(row, selection, projects = []) {
  if (row.scope !== 'global') return [{ project: row.project || row.context || '' }];
  if (selection === model.ALL) return model.callsFor(model.ALL, projects).map((c) => ({ project: c.project }));
  return [{ project: selection || '' }];
}

const clip = (text, n) => {
  const s = String(text || '').replace(/\s+/g, ' ').trim();
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
};

/**
 * The graph data of `memory_neighbours` answers (`[{call, rows}]`, each `{memory, nodes, edges,
 * memories}` or `{error}`): sessions and facts as nodes with the rows their cards show (the
 * profile's `session` and `fact` templates, plus the project's name), the wrote (session -> a
 * memory it wrote, with its tool calls) / about / contains / edited edges. An edge whose ends are neither in these nodes nor in `known` (the ids drawn
 * already) is left out.
 * @param {Array} answers
 * @param {Array} projects
 * @param {Set<string>} known
 * @returns {{nodes: Array, edges: Array}}
 */
export function neighbourGraph(answers, projects = [], known = new Set()) {
  const nodes = [];
  const ids = new Set();
  const edges = [];
  const edgeIds = new Set();
  for (const { rows: answer } of answers || []) {
    if (!answer || answer.error) continue;
    for (const raw of answer.nodes || []) {
      const id = String(raw.id);
      if (ids.has(id) || known.has(id)) continue;
      ids.add(id);
      const projectName = model.projectName(raw.project, projects);
      if (raw.label === 'session') {
        const data = {
          id, headline: raw.name || '', title: raw.title || '', started_at: raw.started_at || '', harness: raw.harness || '',
          turns: raw.turns || 0, project: raw.project || '', project_name: projectName, summary: raw.summary || '',
        };
        nodes.push({ id, label: 'session', caption: clip(raw.name || id, 40), data, classes: [] });
      } else if (raw.label === 'fact') {
        const data = {
          id, fact: raw.summary || raw.name || '', kind: raw.fact_kind || '', why: raw.why || '', at: raw.at || '',
          headline: raw.headline || '', session: raw.session || '', project: raw.project || '', project_name: projectName,
        };
        nodes.push({ id, label: 'fact', caption: clip(raw.summary || raw.name || id, 40), data, classes: raw.fact_kind ? [`fact-${raw.fact_kind.replace(/_/g, '-')}`] : [] });
      } else {
        nodes.push({ id, label: raw.label || 'vertex', caption: clip(raw.name || id, 40), data: { ...raw, project_name: projectName }, classes: [] });
      }
    }
  }
  const has = (id) => ids.has(id) || known.has(id);
  for (const { rows: answer } of answers || []) {
    if (!answer || answer.error) continue;
    for (const raw of answer.edges || []) {
      const id = String(raw.id);
      if (edgeIds.has(id) || !has(String(raw.source)) || !has(String(raw.target))) continue;
      edgeIds.add(id);
      const data = { label: raw.label, title: `${slugOfId(raw.source)} ${EDGE_WORDS[raw.label] || raw.label} ${slugOfId(raw.target)}` };
      if (raw.label === 'wrote') {
        // session -> a memory it wrote: the tool calls (remember, pin, settle, ...) and when.
        const actions = Array.isArray(raw.actions) ? raw.actions.join(', ') : '';
        data.title = `wrote ${slugOfId(raw.target)}${actions ? ` (${actions})` : ''}`;
        data.actions = actions;
        data.at = raw.at || '';
      }
      edges.push({ id, source: String(raw.source), target: String(raw.target), label: raw.label, data });
    }
  }
  return { nodes, edges };
}

/**
 * The toast of an Expand that added nothing: an imported memory was written by no session (its
 * file is named); any other memory was written by no indexed session (a session is linked when
 * it is indexed, after it ended; a memory written in the UI has no session) and nothing is
 * about it. `answers` are the `memory_neighbours` answers.
 */
export function emptyExpandText(slug, answers) {
  const file = (answers || []).map((a) => a && a.rows && a.rows.imported).find((f) => f);
  if (file) {
    return `"${slug}" was imported from ${file}, not written in a session: no session or fact is linked to it.`;
  }
  return `No indexed session wrote "${slug}" and nothing is linked to it (a session is linked once it ended and was indexed; a memory written here in the UI has no session).`;
}

/**
 * The buttons of a selected element's card (`api.graph` `card.actions`): a memory opens in the
 * Memory tab, pins or unpins, goes to Conflicts when it has one, and is forgotten (never an
 * imported one); an open conflict is settled as in the Conflicts tab. Sessions and facts have
 * none (read-only).
 */
export function cardActions(selection) {
  if (!selection) return [];
  const row = selection.data || {};
  if (selection.kind === 'node' && selection.label === 'memory') {
    const out = [{ id: 'open', text: 'Open in Memory', kind: 'primary' }];
    out.push(row.pinned
      ? { id: 'unpin', text: 'Unpin' }
      : { id: 'pin', text: 'Pin', title: 'Load it at the start of every session' });
    if (row.has_conflict) out.push({ id: 'conflicts', text: 'Settle in Conflicts' });
    if (!row.imported) out.push({ id: 'forget', text: 'Forget', kind: 'danger' });
    return out;
  }
  if (selection.kind === 'edge' && selection.label === 'conflicts_with') {
    return [
      { id: 'from-replaces', text: `${row.from} replaces ${row.to}` },
      { id: 'to-replaces', text: `${row.to} replaces ${row.from}` },
      { id: 'both', text: 'Both hold' },
      { id: 'conflicts', text: 'Open Conflicts' },
    ];
  }
  return [];
}

/**
 * The write of a card button (`cardActions` ids): `{call, args, done, confirm?}` for a memory
 * function, `{navigate: [tab, params]}` for a tab, or null when the button writes nothing here.
 * A forget asks first (`confirm` text) and is refused for an imported memory.
 */
export function actionPlan(id, selection) {
  const row = (selection && selection.data) || {};
  if (selection && selection.label === 'memory') {
    if (id === 'open') return { navigate: ['memory', { slug: row.slug, project: row.context || '' }] };
    if (id === 'conflicts') return { navigate: ['conflicts', {}] };
    if (id === 'pin' || id === 'unpin') {
      return {
        call: 'pin',
        args: { slug: row.slug, pinned: id === 'pin', order: null, project: row.context || '' },
        done: id === 'pin' ? `Pinned "${row.slug}": every session starts with it.` : `Unpinned "${row.slug}".`,
      };
    }
    if (id === 'forget') {
      if (row.imported) return { refused: `"${row.slug}" was imported from ${row.file || 'a file'}: edit or delete that file instead.` };
      return {
        call: 'forget',
        args: { slug: row.slug, source: 'ui', project: row.context || '' },
        confirm: `Forget the memory "${row.slug}" (${row.where})? Agents will no longer find it; this cannot be undone here.`,
        done: `Forgot "${row.slug}".`,
      };
    }
    return null;
  }
  if (selection && selection.label === 'conflicts_with') {
    if (id === 'conflicts') return { navigate: ['conflicts', {}] };
    const project = row.context || '';
    if (id === 'from-replaces') return { call: 'settle', args: { slug: row.from, decision: 'supersedes', other: row.to, project }, done: `Recorded: ${row.from} replaces ${row.to}.` };
    if (id === 'to-replaces') return { call: 'settle', args: { slug: row.to, decision: 'supersedes', other: row.from, project }, done: `Recorded: ${row.to} replaces ${row.from}.` };
    if (id === 'both') return { call: 'settle', args: { slug: row.from, decision: 'compatible', other: row.to, project }, done: 'Recorded: both hold.' };
  }
  return null;
}

/**
 * The graph with the memory node `id` carrying its full card row (`full`: `model.memoryRow` of the
 * `memory` answer: the body, start line, links, about, decision history...), so the Graph tab's
 * side card shows the whole memory, as the Memory tab does. The node's own fields win (they style
 * the drawing and come from the same answer set); a field the node lacks or has empty is taken
 * from `full`. Returns the same graph when no such node is drawn.
 * @returns {{nodes: Array, edges: Array}}
 */
export function withFullMemory(graph, id, full) {
  if (!full || !graph.nodes.some((n) => n.id === id)) return graph;
  const empty = (v) => v === undefined || v === null || v === '' || (Array.isArray(v) && !v.length);
  return {
    ...graph,
    nodes: graph.nodes.map((n) => {
      if (n.id !== id) return n;
      const data = { ...n.data };
      for (const [key, value] of Object.entries(full)) if (empty(data[key]) && !empty(value)) data[key] = value;
      return { ...n, data };
    }),
  };
}

/** The size range of memory nodes by their links (px; the profile's default memory size is 26). */
export const MEMORY_SIZE = Object.freeze({ min: 18, max: 56 });

/**
 * The size of each memory node by its weight: how many edges link it to other memories drawn
 * (supersedes, conflicts_with, narrows, relates_to; sessions and facts added by expanding do not
 * count, so a node keeps its size while others are expanded). A square-root scale from
 * `MEMORY_SIZE.min` (no link) to `MEMORY_SIZE.max` (the most linked memory), so one hub does not
 * flatten the rest. Empty when no memory has a link (every node keeps the profile's size).
 * @returns {Map<string, number>} node id -> size
 */
export function memorySizes(graph) {
  const memories = new Set(graph.nodes.filter((n) => n.label === 'memory').map((n) => n.id));
  const degree = new Map([...memories].map((id) => [id, 0]));
  for (const e of graph.edges) {
    if (e.source === e.target || !memories.has(e.source) || !memories.has(e.target)) continue;
    degree.set(e.source, degree.get(e.source) + 1);
    degree.set(e.target, degree.get(e.target) + 1);
  }
  const most = Math.max(0, ...degree.values());
  const out = new Map();
  if (!most) return out;
  for (const [id, d] of degree) {
    out.set(id, Math.round(MEMORY_SIZE.min + (MEMORY_SIZE.max - MEMORY_SIZE.min) * Math.sqrt(d / most)));
  }
  return out;
}

/** The Expand button of a selected node's card: memories only (their sessions and facts). */
export function expandText(selection) {
  return selection && selection.kind === 'node' && selection.label === 'memory' ? 'Expand: sessions and facts' : false;
}

/** The note under the graph: how many memories, how many replaced, how many in a conflict. */
export function graphSummary(graph) {
  const memories = graph.nodes.filter((n) => n.label === 'memory');
  const replaced = memories.filter((n) => n.data.superseded_by).length;
  const conflicts = graph.edges.filter((e) => e.label === 'conflicts_with').length;
  const others = graph.nodes.length - memories.length;
  const parts = [`${memories.length} ${memories.length === 1 ? 'memory' : 'memories'}`];
  if (replaced) parts.push(`${replaced} replaced`);
  if (conflicts) parts.push(`${conflicts} open ${conflicts === 1 ? 'conflict' : 'conflicts'}`);
  if (others) parts.push(`${others} ${others === 1 ? 'session or fact' : 'sessions and facts'} expanded`);
  return parts.join(' · ');
}
