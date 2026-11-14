// The memory profile's pure logic (no DOM, no Vue): the project selection, the rows the cards
// show, "All projects" merging, the start context's tiers and warnings, the arguments of the
// writes and the checks before them. Tested with `node --test` (ui/tests/model.test.mjs).
//
// Every write goes through the memory functions the agent uses (remember, forget, pin, settle)
// or the UI's set_order, with `source: "ui"` where the function takes it; the module never
// builds a query text.

/** The selection "All projects" (the same calls once per project, plus the global ones once). */
export const ALL = '*';
/** The selection "Global" (the global memories only: project ""). */
export const GLOBAL = '';

/** A slug as the memory functions accept it. */
export const SLUG = /^[a-z0-9-]+$/;

/** The kinds of a memory, with the words the UI uses. */
export const KINDS = Object.freeze([
  { id: 'feedback', text: 'how to work (feedback)' },
  { id: 'user', text: 'about you (user)' },
  { id: 'project', text: 'project fact or decision' },
  { id: 'reference', text: 'where things are (reference)' },
]);

/** The source filter of the memory list (`memory_list`'s `source_kind`). */
export const SOURCES = Object.freeze([
  { id: 'agent', text: 'written by the agent' },
  { id: 'ui', text: 'written here' },
  { id: 'file', text: 'imported file (read-only)' },
  { id: 'other', text: 'other memory tool (read-only)' },
]);

/** The tiers of the start context, in delivery order (`start_context::entries_json` `tier`). */
export const TIERS = Object.freeze([
  { id: 'pinned', title: 'Pinned' },
  { id: 'feedback', title: 'How to work (feedback)' },
  { id: 'user', title: 'About you (user)' },
]);

/** The last part of a path (either separator). */
export function baseName(path) {
  const text = String(path || '').replace(/[\\/]+$/, '');
  const parts = text.split(/[\\/]/);
  return parts[parts.length - 1] || text;
}

/** The name of a selection: a project's name, "Global" or "All projects". */
export function selectionName(selection, projects = []) {
  if (selection === ALL) return 'All projects';
  if (selection === GLOBAL) return 'Global';
  const found = projects.find((p) => p.id === selection);
  return found ? found.name : baseName(selection);
}

/** The name of the project a row belongs to (`project` id, "" = global). */
export function projectName(id, projects = []) {
  if (!id) return 'global';
  const found = projects.find((p) => p.id === id);
  return found ? found.name : baseName(id);
}

/** A valid selection out of a stored one: unknown projects fall back to All. */
export function validSelection(stored, projects = []) {
  if (stored === ALL || stored === GLOBAL) return stored;
  if (typeof stored === 'string' && projects.some((p) => p.id === stored)) return stored;
  return ALL;
}

/**
 * The `project` arguments of one read for a selection: one project, the global memories, or
 * every project plus the global ones once (`{project, only}`: `only` is the scope to keep from
 * that call's rows, so a global memory is not listed once per project).
 */
export function callsFor(selection, projects = []) {
  if (selection === ALL) {
    return [...projects.map((p) => ({ project: p.id, only: 'project' })), { project: GLOBAL, only: 'global' }];
  }
  return [{ project: selection, only: null }];
}

/** The source of a memory in plain words. */
export function sourceText(row) {
  const kind = sourceKind(row);
  if (kind === 'ui') return 'written here';
  if (kind === 'agent') return 'agent';
  if (kind === 'other') return `${baseName(row.source)} (other memory tool, read-only)`;
  return `${baseName(row.source)} (read-only)`;
}

/** agent | ui | file | other, from `source_kind` or from `source`/`imported_at`. */
export function sourceKind(row) {
  if (row.source_kind) return row.source_kind;
  if (row.imported_at) return 'file';
  if (row.source === 'ui') return 'ui';
  // An answer without `imported_at` names an imported memory's file in `source`.
  if (row.source && !['remember', 'user'].includes(row.source)) return 'file';
  return 'agent';
}

/** Whether a memory is read-only in the UI (imported from a file or another memory tool). */
export function isImported(row) {
  return !!row.imported_at || sourceKind(row) === 'file' || sourceKind(row) === 'other';
}

/** `SUPERSEDED by x on d: ` taken off a summary or hook the memory functions marked. */
export function unmarked(text) {
  return String(text || '').replace(/^SUPERSEDED by [a-z0-9-]+ on [0-9-]*: /, '');
}

/** A list of slugs or ids (or rows with one) as "a, b, c"; a text stays as it is. */
function listText(value) {
  if (!Array.isArray(value)) return value === undefined || value === null ? '' : value;
  return value.map((x) => (x !== null && typeof x === 'object' ? x.slug || x.id || x.name || '' : String(x)))
    .filter(Boolean).join(', ');
}

/**
 * The row of a memory card (from `memory_list`, `memories` or `memory`): the template's
 * fields (`where`, `source_text`, `imported`, `has_conflict`, ...) added, every value plain.
 */
export function memoryRow(row, projects = [], context = '') {
  const imported = isImported(row);
  const conflicts = Array.isArray(row.conflicts_with)
    ? row.conflicts_with.map((c) => c.slug)
    : Array.isArray(row.open_conflict_with)
      ? row.open_conflict_with.map((c) => (typeof c === 'string' ? c : c.slug))
      : Array.isArray(row.conflicts) ? row.conflicts : [];
  const supersededBy = Array.isArray(row.superseded_by)
    ? (row.superseded_by[0] && row.superseded_by[0].slug) || ''
    : row.superseded_by || '';
  const projectId = row.project !== undefined ? row.project : (row.scope === 'global' ? '' : context);
  const out = {
    ...row,
    summary: unmarked(row.summary),
    hook: unmarked(row.hook),
    project: projectId || '',
    where: row.scope === 'global' ? 'global' : `project ${projectName(projectId, projects)}`,
    source_text: sourceText(row),
    source_kind: sourceKind(row),
    imported,
    file: imported ? row.source : '',
    superseded_by: supersededBy,
    has_conflict: conflicts.length > 0,
    always: row.always !== undefined ? row.always
      : !supersededBy && (row.pinned === true || row.kind === 'feedback' || row.kind === 'user'),
    conflict_text: conflicts.join(', '),
    chain_text: Array.isArray(row.chain) && row.chain.length > 1 ? row.chain.join(' → ') : '',
    // Lists as text: a card field shows a list value as JSON.
    links: listText(row.links),
    linked_from: listText(row.linked_from),
    about: listText(row.about),
    // The project the writes of this card pass: its own project, else the one looked at.
    context: row.scope === 'global' ? (context === ALL ? GLOBAL : context) : projectId || context,
  };
  if (out.context === ALL) out.context = GLOBAL;
  return out;
}

/**
 * The rows of one read per call (`callsFor`) merged: each call's rows of its `only` scope, in
 * call order (projects, then the global ones), each tagged with the call's project.
 */
export function mergeRows(answers) {
  const out = [];
  const seen = new Set();
  for (const { call, rows } of answers) {
    for (const row of rows || []) {
      if (call.only && row.scope && row.scope !== call.only) continue;
      const key = `${row.scope === 'global' ? '' : (row.project !== undefined ? row.project : call.project)}\u0000${row.slug || row.id || ''}`;
      if (seen.has(key)) continue;
      seen.add(key);
      out.push({ ...row, project: row.project !== undefined ? row.project : (row.scope === 'global' ? '' : call.project) });
    }
  }
  return out;
}

/** Search hits of several calls: merged as `mergeRows`, best score first, at most `k`. */
export function mergeHits(answers, k) {
  return mergeRows(answers)
    .sort((a, b) => (Number(b.score) || 0) - (Number(a.score) || 0))
    .slice(0, k);
}

/**
 * The overview of "All projects" from the overview of each project and of the global memories:
 * every project's call counts the global memories too, so they are taken off each and added
 * once.
 */
export function sumOverviews(perProject, global) {
  const out = { ...global };
  for (const one of perProject) {
    for (const [key, value] of Object.entries(one)) {
      if (typeof value !== 'number') continue;
      const base = typeof global[key] === 'number' ? global[key] : 0;
      out[key] = (typeof out[key] === 'number' ? out[key] : 0) + value - base;
    }
  }
  return out;
}

/** The entries of the start context grouped by tier, in delivery order. */
export function tiers(entries = []) {
  return TIERS.map((tier) => ({ ...tier, entries: entries.filter((e) => e.tier === tier.id) }))
    .filter((tier) => tier.entries.length > 0);
}

/**
 * What is cut first when the start context runs out of room: the text keeps the memories in
 * delivery order (`start_context::sort`: pinned, then feedback, then user) and cuts from the
 * end, so About you goes first, then How to work, then Pinned.
 */
export const CUT_ORDER_NOTE = 'When the budget runs out, the end of the delivery order is cut first: About you, then How to work, then Pinned.';

/**
 * The sections of the start context's text, in the order the agent reads them. In How to work
 * and About you every memory is loaded anyway (`pinBox` false: no pin checkbox, a pin there
 * only moves a memory to the front of the cut order, `keptFirst`); the note says so.
 */
export const SECTIONS = Object.freeze([
  Object.freeze({
    id: 'How to work', title: 'How to work (feedback)', pinBox: false,
    note: `Always loaded: every "how to work" memory reaches each session. The order decides what is cut first when space runs out. ${CUT_ORDER_NOTE}`,
  }),
  Object.freeze({
    id: 'About you', title: 'About you (user)', pinBox: false,
    note: `Always loaded: every "about you" memory reaches each session. The order decides what is cut first when space runs out. ${CUT_ORDER_NOTE}`,
  }),
  Object.freeze({ id: 'Pinned', title: 'Pinned', pinBox: true, note: '' }),
]);

/**
 * The entries of the start context grouped exactly as its text lists them
 * (`start_context::entries_json` `group`): How to work, About you, Pinned; within a section in
 * delivery order (the pinned ones first). The order groups of `set_order` stay the tiers.
 */
export function sections(entries = []) {
  return SECTIONS.map((section) => ({ ...section, entries: entries.filter((e) => e.group === section.id) }))
    .filter((section) => section.entries.length > 0);
}

/**
 * The pin controls of an entry in its section: the checkbox only in a section with `pinBox`;
 * elsewhere a pinned entry shows the "kept first" mark with a button that removes it.
 */
export function pinControls(section, entry) {
  const box = Boolean(section && section.pinBox);
  return { checkbox: box, keptFirst: !box && Boolean(entry && entry.pinned) };
}

/** The tooltip of the "kept first" mark. */
export const KEPT_FIRST_TITLE = 'Marked pinned: it is listed first here and cut last, with the Pinned section. It is loaded either way; remove the mark to let it follow this section\'s order.';

/**
 * The labels of an entry that does not fully reach the agent: "cut: no room" (over the budget,
 * not in the text) and "shortened" (its line's text cut at the line limit,
 * `start_context::entries_json` `shortened`/`shown_hook_chars`); each with what the agent gets.
 */
export function reachLabels(entry) {
  const out = [];
  if (!entry) return out;
  if (entry.shown === false) {
    out.push({
      id: 'cut', text: 'cut: no room',
      title: `Not in the start context (over the budget): the agent is only told that more memories exist and can read this one with memory("${entry.slug}") or find it with memories(text).`,
    });
  } else if (entry.shortened) {
    const n = Number(entry.shown_hook_chars) || 0;
    out.push({
      id: 'shortened', text: 'shortened',
      title: `The agent gets only the first ${n ? grouped(n) + ' ' : ''}characters of this text, then "…"; the full memory through memory("${entry.slug}").`,
    });
  }
  return out;
}

/**
 * The slugs of `slug`'s tier in their new order after a move by `delta` (-1 up, +1 down): it
 * swaps places with the next entry of the same tier in the same text section, or null when it
 * cannot move (first/last there, or unknown).
 */
export function moved(entries, slug, delta) {
  const entry = entries.find((e) => e.slug === slug);
  if (!entry) return null;
  const tier = entries.filter((e) => e.tier === entry.tier);
  const at = tier.indexOf(entry);
  let to = at + delta;
  while (to >= 0 && to < tier.length && tier[to].group !== entry.group) to += delta;
  if (to < 0 || to >= tier.length) return null;
  const slugs = tier.map((e) => e.slug);
  [slugs[at], slugs[to]] = [slugs[to], slugs[at]];
  return slugs;
}

/**
 * The warnings of the Always loaded tab: pinned memories cut for lack of room, pinned memories
 * in an open conflict, pinned memories that were replaced (they never start a session).
 * `entries`: the start context's; `rows`: `memory_list` with the replaced ones.
 */
export function warnings(entries = [], rows = []) {
  const out = [];
  const cut = entries.filter((e) => e.pinned && !e.shown);
  if (cut.length) {
    out.push(`The budget is too small for the pinned memories: ${cut.length} pinned ${cut.length === 1 ? 'memory is' : 'memories are'} cut (${cut.map((e) => e.slug).join(', ')}). Raise the budget or unpin some.`);
  }
  for (const row of rows) {
    if (row.pinned && row.conflicts && row.conflicts.length) {
      out.push(`Pinned "${row.slug}" has an open conflict with ${row.conflicts.join(', ')}: settle it in Conflicts.`);
    }
  }
  for (const row of rows) {
    if (row.pinned && row.superseded_by) {
      out.push(`Pinned "${row.slug}" was replaced by "${row.superseded_by}" and is no longer loaded: unpin it or forget it.`);
    }
  }
  return out;
}

/** The budget bar's numbers: used characters, budget, percent (0..100), over. */
export function budgetBar(context) {
  const chars = Number(context && context.chars) || 0;
  const budget = Number(context && context.budget) || 0;
  const percent = budget > 0 ? Math.min(100, Math.round((chars / budget) * 100)) : 0;
  return { chars, budget, percent, cut: Number(context && context.total) - Number(context && context.shown) || 0 };
}

/** A number with thin grouping (6 120). */
export function grouped(n) {
  return String(Math.round(Number(n) || 0)).replace(/\B(?=(\d{3})+(?!\d))/g, ' ');
}

/** The problems of a memory form (empty: it can be saved). */
export function formProblems(form, { creating = false } = {}) {
  const problems = [];
  if (creating && !SLUG.test(form.slug || '')) problems.push('The name (slug) takes lowercase letters, digits and "-" only.');
  if (!KINDS.some((k) => k.id === form.kind)) problems.push('Choose a kind.');
  if (!String(form.summary || '').trim()) problems.push('The summary (one line) is needed.');
  if (!String(form.body || '').trim()) problems.push('The text is needed.');
  return problems;
}

/** The form of an existing memory (`memory` answer): title and hook only when set. */
export function formOf(memory) {
  return {
    slug: memory.slug,
    kind: memory.kind,
    scope: memory.scope,
    title: memory.title && memory.title !== memory.slug ? memory.title : '',
    summary: unmarked(memory.summary),
    hook: memory.hook && unmarked(memory.hook) !== unmarked(memory.summary) ? unmarked(memory.hook) : '',
    body: memory.body || '',
  };
}

/**
 * The `remember` arguments of a form (create or edit) in project `project` ("" = global only):
 * source "ui", why/how_to_apply empty (the text holds them), pin and order kept (null), links
 * and what it is about kept from the memory being edited.
 */
export function rememberArgs(form, project, memory = null) {
  return {
    slug: form.slug,
    kind: form.kind,
    summary: String(form.summary).trim(),
    body: String(form.body).trim(),
    why: '',
    how_to_apply: '',
    title: String(form.title || '').trim(),
    hook: String(form.hook || '').trim(),
    pinned: null,
    order: null,
    links: memory && Array.isArray(memory.links) ? memory.links : [],
    about: memory && Array.isArray(memory.about) ? memory.about : [],
    supersedes: [],
    from_file: null,
    scope: form.scope === 'global' || !project ? 'global' : 'project',
    source: 'ui',
    project: project || '',
  };
}

/** Whether a memory changed since it was opened for editing (its `updated_at` moved). */
export function changedMeanwhile(opened, now) {
  if (!now || now.error) return true;
  return String(opened && opened.updated_at) !== String(now.updated_at);
}

/**
 * The choices for one nearest memory after a save (`remember`'s `nearest` + `check`), as
 * `settle` decisions in plain words. A project memory next to a global one can only narrow it.
 */
export function nearestChoices(saved, near) {
  const crossScope = saved.scope !== near.scope;
  const choices = [{ id: 'compatible', text: 'Both hold', title: 'They do not contradict each other.' }];
  if (crossScope && saved.scope === 'project') {
    choices.push({ id: 'narrows', text: `Narrows the global "${near.slug}"`, title: 'This project\'s memory makes an exception to the global one.' });
  } else if (!crossScope) {
    choices.push({ id: 'supersedes', text: `Replaces "${near.slug}"`, title: 'The new memory is the later decision; the old one stays, marked as replaced.' });
    choices.push({ id: 'conflict', text: 'They contradict', title: 'Record an open conflict and decide later in Conflicts.' });
  }
  return choices;
}

/** The sessions grouped by their start day (YYYY-MM-DD), newest first. */
export function sessionDays(rows = []) {
  const days = new Map();
  for (const row of rows) {
    const day = String(row.started_at || '').slice(0, 10) || 'unknown';
    if (!days.has(day)) days.set(day, []);
    days.get(day).push(row);
  }
  return [...days.entries()].map(([day, sessions]) => ({ day, sessions }));
}

/** Sessions of several calls merged, newest first, at most `limit`. */
export function mergeSessions(answers, limit) {
  const all = [];
  const seen = new Set();
  for (const { call, rows } of answers) {
    for (const row of rows || []) {
      if (seen.has(row.id)) continue;
      seen.add(row.id);
      all.push({ ...row, project: call.project });
    }
  }
  return all.sort((a, b) => String(b.started_at).localeCompare(String(a.started_at))).slice(0, limit);
}

/** Open conflicts of several calls merged (one per pair of memories). */
export function mergeConflicts(answers) {
  const open = [];
  const unchecked = [];
  const seenPairs = new Set();
  const seenUnchecked = new Set();
  for (const { call, rows } of answers) {
    const answer = rows || {};
    for (const pair of answer.open || []) {
      const key = [pair.a.slug, pair.a.scope, pair.b.slug, pair.b.scope].join('\u0000');
      if (seenPairs.has(key)) continue;
      seenPairs.add(key);
      open.push({ ...pair, project: call.project });
    }
    for (const memory of answer.unchecked || []) {
      if (call.only && memory.scope !== call.only) continue;
      const key = `${memory.scope === 'global' ? '' : call.project}\u0000${memory.slug}`;
      if (seenUnchecked.has(key)) continue;
      seenUnchecked.add(key);
      unchecked.push({ ...memory, project: call.project });
    }
  }
  const total = answers.length === 1 && answers[0].rows && typeof answers[0].rows.unchecked_total === 'number'
    ? answers[0].rows.unchecked_total
    : unchecked.length;
  const rules = mergeRulePairs(answers);
  const rulesTotal = answers.length === 1 && answers[0].rows && typeof answers[0].rows.rules_unchecked_total === 'number'
    ? answers[0].rows.rules_unchecked_total
    : rules.filter((r) => r.status === 'unchecked').length;
  // The check against instruction files needs an embedding model: off when an answer says so.
  const rulesOff = answers.some(({ rows }) => Boolean(rows && rows.rules_off));
  return { open, unchecked, unchecked_total: total, rules, rules_unchecked_total: rulesTotal, rules_off: rulesOff };
}

/**
 * The pairs of a memory and a line of an instruction file of the `conflicts` answers
 * (`rules_open`: open conflicts, `rules_unchecked`: candidates still to check), each once, the
 * open conflicts first, with `status` ('conflict' | 'unchecked') and the call's project.
 */
export function mergeRulePairs(answers) {
  const seen = new Set();
  const pairs = [];
  for (const status of ['conflict', 'unchecked']) {
    for (const { call, rows } of answers) {
      const answer = rows || {};
      for (const pair of (status === 'conflict' ? answer.rules_open : answer.rules_unchecked) || []) {
        if (!pair || !pair.memory || !pair.rule) continue;
        // "All projects": a project's call keeps the pairs with a project side (a global memory
        // against a project's line is that project's); the global call keeps the rest.
        if (call.only === 'project' && pair.memory.scope === 'global' && pair.rule.scope === 'global') continue;
        const key = rulePairKey(pair, call.project);
        if (seen.has(key)) continue;
        seen.add(key);
        pairs.push({ ...pair, status, project: call.project });
      }
    }
  }
  return pairs;
}

/** The line shown when the check of memories against instruction files is off (no embedding model). */
export const RULES_OFF_TEXT = 'Off: this check needs an embedding model. Install one with `graphersal-memory install --model <repo>` (the wizard offers them); it starts by itself afterwards.';

/** "N pairs waiting for the agent's check": unchecked memory / instruction line pairs are never called conflicts. */
export function waitingText(n) {
  return `${n} ${n === 1 ? 'pair' : 'pairs'} waiting for the agent's check`;
}

/** The key of a memory / instruction line pair (a pair of global ones is the same in every project). */
export function rulePairKey(pair, project = pair.project) {
  const memory = pair.memory.scope === 'global' && pair.rule.scope === 'global' ? `global/${pair.memory.slug}` : `${project}/${pair.memory.slug}`;
  return `${pair.rule.rule}|${memory}`;
}

/** "file:line" of an instruction line. */
export function fileLine(rule) {
  return `${rule.file}:${rule.line}`;
}

/** The harnesses that load an instruction file, readable. */
export function loadedByText(rule) {
  const names = { 'claude-code': 'Claude Code', opencode: 'opencode', codex: 'Codex', cursor: 'Cursor' };
  const list = String(rule.loaded_by || '').split(',').filter(Boolean).map((id) => names[id] || id);
  return list.length ? `loaded by ${list.join(', ')}` : '';
}

/**
 * What the buttons of a memory / instruction line pair do: `settle` arguments for "Both hold"
 * and "File is outdated" (recorded as a conflict, so it stays listed until the line changes),
 * whether "Memory is outdated" may forget it (never an imported memory: its file is the one to
 * edit), and the texts the UI shows.
 */
export function ruleActions(pair) {
  const imported = isImported(pair.memory);
  const where = `${pair.rule.file} line ${pair.rule.line}`;
  return {
    compatible: { slug: pair.memory.slug, decision: 'compatible', other: '', rule: pair.rule.rule, project: pair.project },
    conflict: { slug: pair.memory.slug, decision: 'conflict', other: '', rule: pair.rule.rule, project: pair.project },
    canForget: !imported,
    forgetTitle: imported
      ? `Imported from ${pair.memory.source}: edit or delete that file instead (an imported memory is read-only here).`
      : 'Forget this memory: the instruction file\'s line stays the rule.',
    fileText: `File is outdated: edit ${where}`,
    fileNote: `Edit ${where} yourself: graphersal-memory never changes instruction files. The conflict stays listed and closes by itself when that line changes or goes (at the next session start, or with \`graphersal-memory import\`).`,
  };
}

/** The status lines of `/api/ui/status` that need the user: problems and actions. */
export function healthProblems(status) {
  const lines = [...((status && status.doctor) || [])];
  return lines.filter((line) => /PROBLEM|ACTION NEEDED/.test(line)).map((line) => line.trim());
}

/** The count of setup problems in a few words ("1 setup problem", "3 setup problems"). */
export function problemsText(count) {
  return `${count} setup ${count === 1 ? 'problem' : 'problems'}`;
}

/** The daemon's state in a few words. */
export function daemonText(status) {
  const daemon = status && status.daemon;
  if (!daemon || daemon.ok !== true) return 'daemon not running';
  const secs = Number(daemon.uptime_secs) || 0;
  const up = secs >= 86400 ? `${Math.floor(secs / 86400)} d` : secs >= 3600 ? `${Math.floor(secs / 3600)} h` : `${Math.max(1, Math.floor(secs / 60))} min`;
  return `daemon ${daemon.daemon_version}, up ${up}`;
}
