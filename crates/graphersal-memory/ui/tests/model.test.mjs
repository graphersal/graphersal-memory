// node --test crates/graphersal-memory/ui/tests/model.test.mjs
// The memory profile's pure logic (ui/model.js): selection, card rows, "All projects" merging,
// the start context's tiers, moves and warnings, the write arguments and their checks.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import * as m from '../model.js';

const projects = [
  { id: '/w/app', name: 'app', paths: ['/w/app'] },
  { id: 'github.com/acme/lib', name: 'lib', paths: ['/w/lib'] },
];

test('a selection is a known project, Global or All; calls cover All once per project plus global', () => {
  assert.equal(m.validSelection('/w/app', projects), '/w/app');
  assert.equal(m.validSelection('/gone', projects), m.ALL);
  assert.equal(m.validSelection('', projects), m.GLOBAL);
  assert.equal(m.validSelection(null, projects), m.ALL);
  assert.deepEqual(m.callsFor('/w/app', projects), [{ project: '/w/app', only: null }]);
  assert.deepEqual(m.callsFor(m.ALL, projects), [
    { project: '/w/app', only: 'project' },
    { project: 'github.com/acme/lib', only: 'project' },
    { project: '', only: 'global' },
  ]);
  assert.equal(m.selectionName(m.ALL, projects), 'All projects');
  assert.equal(m.selectionName('', projects), 'Global');
  assert.equal(m.selectionName('github.com/acme/lib', projects), 'lib');
});

test('All projects lists a global memory once and each project memory with its project', () => {
  const g = { slug: 'never-push', scope: 'global', project: '' };
  const answers = [
    { call: { project: '/w/app', only: 'project' }, rows: [{ slug: 'a', scope: 'project', project: '/w/app' }, g] },
    { call: { project: 'github.com/acme/lib', only: 'project' }, rows: [{ slug: 'a', scope: 'project', project: 'github.com/acme/lib' }, g] },
    { call: { project: '', only: 'global' }, rows: [g] },
  ];
  const rows = m.mergeRows(answers);
  assert.deepEqual(rows.map((r) => `${r.project}:${r.slug}`), ['/w/app:a', 'github.com/acme/lib:a', ':never-push']);
  // Search hits without a project field get the call's project; best score first.
  const hits = m.mergeHits([
    { call: { project: '/w/app', only: 'project' }, rows: [{ slug: 'x', scope: 'project', score: 0.2 }, { slug: 'g', scope: 'global', score: 0.9 }] },
    { call: { project: '', only: 'global' }, rows: [{ slug: 'g', scope: 'global', score: 0.9 }] },
  ], 5);
  assert.deepEqual(hits.map((h) => `${h.project}:${h.slug}`), [':g', '/w/app:x']);
});

test('the overview of All projects counts the global memories once', () => {
  const global = { memories: 2, global_memories: 2, project_memories: 0, sessions: 0, open_conflicts: 1 };
  const a = { memories: 5, global_memories: 2, project_memories: 3, sessions: 4, open_conflicts: 1 };
  const b = { memories: 3, global_memories: 2, project_memories: 1, sessions: 1, open_conflicts: 2 };
  assert.deepEqual(m.sumOverviews([a, b], global), {
    memories: 6, global_memories: 2, project_memories: 4, sessions: 5, open_conflicts: 2,
  });
});

test('a memory row says where it lives and where it came from in plain words', () => {
  const agent = m.memoryRow({ slug: 'a', scope: 'project', project: '/w/app', source: 'remember', source_kind: 'agent' }, projects, m.ALL);
  assert.equal(agent.where, 'project app');
  assert.equal(agent.source_text, 'agent');
  assert.equal(agent.imported, false);
  assert.equal(agent.context, '/w/app');
  const global = m.memoryRow({ slug: 'g', scope: 'global', project: '', source: 'ui' }, projects, '/w/app');
  assert.equal(global.where, 'global');
  assert.equal(global.source_text, 'written here');
  assert.equal(global.context, '/w/app', 'writes from a project pass that project');
  assert.equal(m.memoryRow({ slug: 'g', scope: 'global', project: '' }, projects, m.ALL).context, '');
  const imported = m.memoryRow({
    slug: 'f', scope: 'project', source: '/h/.claude/projects/x/memory/f.md', imported_at: '2026-11-01T00:00:00Z',
    summary: 'SUPERSEDED by g on 2026-11-02: old rule', superseded_by: [{ slug: 'g', at: '2026-11-02' }],
    chain: ['f', 'g'], conflicts_with: [{ slug: 'h' }],
  }, projects, '/w/app');
  assert.equal(imported.imported, true);
  assert.equal(imported.source_text, 'f.md (read-only)');
  assert.equal(imported.file, '/h/.claude/projects/x/memory/f.md');
  assert.equal(imported.summary, 'old rule');
  assert.equal(imported.superseded_by, 'g');
  assert.equal(imported.chain_text, 'f → g');
  assert.equal(imported.has_conflict, true);
  // A conflict side names its file in `source` without `imported_at`.
  assert.equal(m.isImported({ source: '/h/memory/x.md' }), true);
  assert.equal(m.isImported({ source: 'remember' }), false);
  assert.equal(m.isImported({ source: 'ui' }), false);
});

test('the start context groups by tier and moves stay inside a tier', () => {
  const entries = [
    { slug: 'p1', tier: 'pinned', pinned: true, shown: true },
    { slug: 'p2', tier: 'pinned', pinned: true, shown: false },
    { slug: 'f1', tier: 'feedback', shown: true },
    { slug: 'f2', tier: 'feedback', shown: true },
    { slug: 'u1', tier: 'user', shown: false },
  ];
  assert.deepEqual(m.tiers(entries).map((t) => [t.id, t.entries.length]), [['pinned', 2], ['feedback', 2], ['user', 1]]);
  assert.deepEqual(m.moved(entries, 'f2', -1), ['f2', 'f1']);
  assert.equal(m.moved(entries, 'f1', -1), null, 'first of its tier');
  assert.equal(m.moved(entries, 'p2', 1), null, 'never into another tier');
  assert.equal(m.moved(entries, 'nope', 1), null);
  const warnings = m.warnings(entries, [
    { slug: 'p1', pinned: true, conflicts: ['x'] },
    { slug: 'old', pinned: true, superseded_by: 'new' },
    { slug: 'f1', pinned: false, conflicts: ['y'] },
  ]);
  assert.equal(warnings.length, 3, warnings.join('\n'));
  assert.match(warnings[0], /too small.*p2/);
  assert.match(warnings[1], /"p1".*conflict.*x/);
  assert.match(warnings[2], /"old".*replaced by "new"/);
  assert.deepEqual(m.budgetBar({ chars: 6120, budget: 8000, total: 5, shown: 3 }), { chars: 6120, budget: 8000, percent: 77, cut: 2 });
  assert.equal(m.grouped(6120), '6 120');
});

test('an edit sends the memory back as the UI with its links and keeps pin and order', () => {
  const memory = {
    slug: 'use-pnpm', kind: 'feedback', scope: 'project', title: 'use-pnpm', summary: 'Use pnpm',
    hook: 'Use pnpm', body: 'Use pnpm.\n\n**Why:** speed', links: ['other'], about: ['session:x'], updated_at: 't1',
  };
  const form = m.formOf(memory);
  assert.equal(form.title, '', 'a title equal to the slug is no title');
  assert.equal(form.hook, '', 'a hook equal to the summary is no hook');
  const args = m.rememberArgs({ ...form, summary: 'Use pnpm always' }, '/w/app', memory);
  assert.deepEqual(args, {
    slug: 'use-pnpm', kind: 'feedback', summary: 'Use pnpm always', body: 'Use pnpm.\n\n**Why:** speed', why: '',
    how_to_apply: '', title: '', hook: '', pinned: null, order: null, links: ['other'], about: ['session:x'],
    supersedes: [], from_file: null, scope: 'project', source: 'ui', project: '/w/app',
  });
  assert.equal(m.rememberArgs({ ...form, scope: 'global' }, '/w/app', memory).scope, 'global');
  assert.equal(m.rememberArgs(form, '', memory).scope, 'global');
  assert.equal(m.changedMeanwhile(memory, { ...memory }), false);
  assert.equal(m.changedMeanwhile(memory, { ...memory, updated_at: 't2' }), true);
  assert.equal(m.changedMeanwhile(memory, { error: 'no memory' }), true);
  assert.deepEqual(m.formProblems({ slug: 'Bad Slug', kind: 'x', summary: '', body: '' }, { creating: true }).length, 4);
  assert.deepEqual(m.formProblems({ slug: 'ok-slug', kind: 'user', summary: 's', body: 'b' }, { creating: true }), []);
});

test('after a save the nearest memories offer only the decisions settle accepts', () => {
  const ids = (saved, near) => m.nearestChoices(saved, near).map((c) => c.id);
  assert.deepEqual(ids({ scope: 'project' }, { scope: 'project', slug: 'x' }), ['compatible', 'supersedes', 'conflict']);
  assert.deepEqual(ids({ scope: 'project' }, { scope: 'global', slug: 'x' }), ['compatible', 'narrows']);
  assert.deepEqual(ids({ scope: 'global' }, { scope: 'project', slug: 'x' }), ['compatible']);
});

test('sessions are grouped by day and merged newest first; conflicts once per pair', () => {
  const days = m.sessionDays([
    { id: 'a', started_at: '2026-11-12T09:00:00Z' },
    { id: 'b', started_at: '2026-11-12T08:00:00Z' },
    { id: 'c', started_at: '2026-11-10T08:00:00Z' },
  ]);
  assert.deepEqual(days.map((d) => [d.day, d.sessions.length]), [['2026-11-12', 2], ['2026-11-10', 1]]);
  const merged = m.mergeSessions([
    { call: { project: 'p' }, rows: [{ id: 's1', started_at: '2026-11-01' }] },
    { call: { project: 'q' }, rows: [{ id: 's2', started_at: '2026-11-03' }, { id: 's1', started_at: '2026-11-01' }] },
  ], 10);
  assert.deepEqual(merged.map((s) => `${s.project}:${s.id}`), ['q:s2', 'p:s1']);
  const pair = { a: { slug: 'g1', scope: 'global' }, b: { slug: 'g2', scope: 'global' } };
  const conflicts = m.mergeConflicts([
    { call: { project: 'p', only: 'project' }, rows: { open: [pair], unchecked: [{ slug: 'u', scope: 'project' }, { slug: 'gu', scope: 'global' }] } },
    { call: { project: '', only: 'global' }, rows: { open: [pair], unchecked: [{ slug: 'gu', scope: 'global' }] } },
  ]);
  assert.equal(conflicts.open.length, 1);
  assert.deepEqual(conflicts.unchecked.map((u) => u.slug), ['u', 'gu']);
  assert.equal(m.mergeConflicts([{ call: { project: 'p', only: null }, rows: { open: [], unchecked: [], unchecked_total: 14 } }]).unchecked_total, 14);
});

test('health: problems and actions from doctor, the daemon in a few words', () => {
  const status = { daemon: { ok: true, daemon_version: '0.1.0', uptime_secs: 3 * 3600 + 5 }, doctor: ['Claude Code:', '  ok  set up', '  PROBLEM hooks not approved', '  ACTION NEEDED: run x'] };
  assert.deepEqual(m.healthProblems(status), ['PROBLEM hooks not approved', 'ACTION NEEDED: run x']);
  assert.equal(m.daemonText(status), 'daemon 0.1.0, up 3 h');
  assert.equal(m.daemonText(null), 'daemon not running');
});

test('the profile passes the playground\'s own validation in allow-list mode', async () => {
  const profile = JSON.parse(readFileSync(new URL('../profile.json', import.meta.url), 'utf8'));
  assert.equal(profile.files, '/memory-ui/');
  assert.ok(profile.tabs.every((t) => t.module && !t.builtin));
  assert.deepEqual(Object.keys(profile.can), ['theme']);
  // The engine's validator, when the graphersal checkout is next to this repository.
  const engine = process.env.GRAPHERSAL_PLAYGROUND;
  if (engine) {
    const { validateProfile } = await import(`${engine}/js/ui-profiles.js`);
    const valid = validateProfile(profile);
    assert.equal(valid.name, 'memory');
  }
});

test('the Always loaded list follows the text: How to work, About you, Pinned', () => {
  const entries = [
    { slug: 'fp', tier: 'pinned', group: 'How to work', pinned: true, shown: true },
    { slug: 'p1', tier: 'pinned', group: 'Pinned', pinned: true, shown: true },
    { slug: 'p2', tier: 'pinned', group: 'Pinned', pinned: true, shown: true },
    { slug: 'f1', tier: 'feedback', group: 'How to work', shown: true },
    { slug: 'u1', tier: 'user', group: 'About you', shown: true },
  ];
  assert.deepEqual(
    m.sections(entries).map((s) => [s.id, s.entries.map((e) => e.slug)]),
    [['How to work', ['fp', 'f1']], ['About you', ['u1']], ['Pinned', ['p1', 'p2']]],
  );
  // A pinned feedback memory moves only among the pinned ones of its own section.
  assert.equal(m.moved(entries, 'fp', 1), null, 'never past the unpinned or into Pinned');
  assert.equal(m.moved(entries, 'p1', -1), null, 'never into How to work');
  assert.deepEqual(m.moved(entries, 'p2', -1), ['fp', 'p2', 'p1']);
});

test('How to work and About you have no pin checkbox; a pinned one there is "kept first"', () => {
  const entries = [
    { slug: 'fp', tier: 'pinned', group: 'How to work', pinned: true, shown: true },
    { slug: 'f1', tier: 'feedback', group: 'How to work', pinned: false, shown: true },
    { slug: 'u1', tier: 'user', group: 'About you', pinned: false, shown: true },
    { slug: 'p1', tier: 'pinned', group: 'Pinned', pinned: true, shown: true },
  ];
  const [work, about, pinned] = m.sections(entries);
  assert.deepEqual(m.pinControls(work, entries[0]), { checkbox: false, keptFirst: true });
  assert.deepEqual(m.pinControls(work, entries[1]), { checkbox: false, keptFirst: false });
  assert.deepEqual(m.pinControls(about, entries[2]), { checkbox: false, keptFirst: false });
  assert.deepEqual(m.pinControls(pinned, entries[3]), { checkbox: true, keptFirst: false });
  // The notes say they are always loaded and name the cut order of start_context::sort.
  for (const section of [work, about]) {
    assert.match(section.note, /^Always loaded/);
    assert.match(section.note, /About you, then How to work, then Pinned/);
  }
  assert.equal(pinned.note, '');
  assert.match(m.KEPT_FIRST_TITLE, /cut last/);
});

test('memories that do not fully reach the agent are labelled with what it gets instead', () => {
  assert.deepEqual(m.reachLabels({ slug: 'a', shown: true, shortened: false }), []);
  const cut = m.reachLabels({ slug: 'a', shown: false, shortened: true });
  assert.deepEqual(cut.map((l) => l.text), ['cut: no room'], 'a cut entry is not also "shortened"');
  assert.match(cut[0].title, /memory\("a"\)/);
  const short = m.reachLabels({ slug: 'long', shown: true, shortened: true, shown_hook_chars: 299 });
  assert.deepEqual(short.map((l) => l.text), ['shortened']);
  assert.match(short[0].title, /first 299 characters/);
  assert.match(short[0].title, /memory\("long"\)/);
  assert.deepEqual(m.reachLabels(null), []);
});

test('memories against instruction files: merged once, open conflicts first, actions per pair', () => {
  const rule = (id) => ({ rule: id, file: '/w/CLAUDE.md', line: 3, text: 'Always use npm.', scope: 'project', loaded_by: 'claude-code,codex' });
  const pnpm = { slug: 'use-pnpm', scope: 'project', kind: 'feedback', summary: 'pnpm', source: 'remember' };
  const global = { slug: 'dark', scope: 'global', kind: 'user', summary: 'dark', source: 'remember' };
  const userRule = { ...rule('a4'), scope: 'global' };
  const merged = m.mergeConflicts([
    { call: { project: 'p', only: 'project' }, rows: { open: [], unchecked: [], rules_unchecked: [{ memory: pnpm, rule: rule('a1') }, { memory: global, rule: rule('a2') }, { memory: global, rule: userRule }], rules_open: [{ memory: pnpm, rule: rule('a3'), since: '2026-11-13' }] } },
    { call: { project: '', only: 'global' }, rows: { open: [], unchecked: [], rules_unchecked: [{ memory: global, rule: userRule }] } },
  ]);
  assert.deepEqual(merged.rules.map((p) => `${p.status}:${p.rule.rule}:${p.memory.slug}:${p.project}`), ['conflict:a3:use-pnpm:p', 'unchecked:a1:use-pnpm:p', 'unchecked:a2:dark:p', 'unchecked:a4:dark:'], 'a global memory against a project line is that project\'s; two global ones once');
  assert.equal(merged.rules_unchecked_total, 3);
  assert.equal(m.mergeConflicts([{ call: { project: 'p', only: null }, rows: { open: [], unchecked: [], rules_unchecked: [], rules_unchecked_total: 9 } }]).rules_unchecked_total, 9);
  assert.deepEqual(m.mergeConflicts([{ call: { project: 'p' }, rows: { open: [], unchecked: [] } }]).rules, [], 'an older answer has none');
  const pair = merged.rules[1];
  assert.equal(m.fileLine(pair.rule), '/w/CLAUDE.md:3');
  assert.equal(m.loadedByText(pair.rule), 'loaded by Claude Code, Codex');
  const actions = m.ruleActions(pair);
  assert.deepEqual(actions.compatible, { slug: 'use-pnpm', decision: 'compatible', other: '', rule: 'a1', project: 'p' });
  assert.equal(actions.conflict.decision, 'conflict');
  assert.equal(actions.canForget, true);
  assert.equal(actions.fileText, 'File is outdated: edit /w/CLAUDE.md line 3');
  assert.match(actions.fileNote, /never changes instruction files/);
  assert.match(actions.fileNote, /closes by itself when that line changes/);
  const imported = m.ruleActions({ ...pair, memory: { ...pnpm, source: '/home/u/memory/use-pnpm.md', imported_at: '2026-11-01' } });
  assert.equal(imported.canForget, false);
  assert.match(imported.forgetTitle, /use-pnpm\.md/);
  assert.notEqual(m.rulePairKey(merged.rules[2], 'p'), m.rulePairKey(merged.rules[1], 'p'));
  assert.notEqual(m.rulePairKey(merged.rules[2], 'p'), m.rulePairKey(merged.rules[2], 'q'), 'a project line is per project');
  assert.equal(m.rulePairKey(merged.rules[3], 'p'), m.rulePairKey(merged.rules[3], 'q'), 'two global sides are one pair everywhere');
});

test('pairs waiting for the agent\'s check are counted, never called conflicts', () => {
  assert.equal(m.waitingText(1), '1 pair waiting for the agent\'s check');
  assert.equal(m.waitingText(7), '7 pairs waiting for the agent\'s check');
  assert.doesNotMatch(m.waitingText(3), /conflict/);
});

test('memory card lists (links, linked from, about) are text, not JSON', () => {
  const row = m.memoryRow({ slug: 'a', scope: 'project', project: '/w/app', kind: 'project', links: ['b'], linked_from: ['c', 'd', 'e'], about: [{ id: 'file:src/x.rs' }] }, [], '/w/app');
  assert.equal(row.links, 'b');
  assert.equal(row.linked_from, 'c, d, e');
  assert.equal(row.about, 'file:src/x.rs');
  assert.equal(m.memoryRow({ slug: 'a', scope: 'global', kind: 'user' }).linked_from, '');
});
