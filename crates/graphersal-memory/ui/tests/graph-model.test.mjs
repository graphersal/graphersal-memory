// node --test crates/graphersal-memory/ui/tests/graph-model.test.mjs
// The Graph tab's pure logic (ui/graph-model.js): the graph data of memory_graph answers (one
// node per memory across "All projects", the style classes), the neighbours of an expanded
// memory, the card buttons and the writes they make.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import * as gm from '../graph-model.js';
import * as m from '../model.js';

const projects = [
  { id: '/w/app', name: 'app', paths: ['/w/app'] },
  { id: '/w/lib', name: 'lib', paths: ['/w/lib'] },
];

const mem = (over) => ({
  id: `memory:${over.slug}${over.scope === 'global' ? '' : '@' + (over.project || '/w/app')}`,
  scope: 'project', project: '/w/app', kind: 'project', title: over.slug, summary: `${over.slug} summary`,
  source: 'remember', source_kind: 'agent', updated_at: '2026-11-12T10:00:00Z', pinned: false, always: false,
  ...over,
});

test('memory classes: kind, imported, always, superseded, in that order', () => {
  assert.deepEqual(gm.memoryClasses({ kind: 'feedback', always: true }), ['kind-feedback', 'always']);
  assert.deepEqual(gm.memoryClasses({ kind: 'project', imported: true, always: true }), ['kind-project', 'imported', 'always']);
  assert.deepEqual(gm.memoryClasses({ kind: 'user', superseded_by: 'x' }), ['kind-user', 'superseded']);
  assert.deepEqual(gm.memoryClasses({ kind: 'odd' }), []);
});

test('memoryGraph: one node per memory over All projects, rows as the cards show them, decision edges', () => {
  const g = mem({ slug: 'never-push', scope: 'global', project: '', kind: 'feedback', always: true });
  const answers = [
    {
      call: { project: '/w/app', only: 'project' },
      rows: {
        nodes: [mem({ slug: 'use-pnpm' }), mem({ slug: 'use-npm', superseded_by: 'use-pnpm' }), mem({ slug: 'tabs', conflicts: ['spaces'] }),
          mem({ slug: 'spaces', conflicts: ['tabs'] }), { ...g, superseded_by: 'use-pnpm' }],
        edges: [
          { id: 'e1', label: 'supersedes', source: 'memory:use-pnpm@/w/app', target: 'memory:use-npm@/w/app', at: '2026-11-12T10:00:00Z' },
          { id: 'e2', label: 'conflicts_with', source: 'memory:tabs@/w/app', target: 'memory:spaces@/w/app', at: '' },
          { id: 'e3', label: 'narrows', source: 'memory:tabs@/w/app', target: 'memory:never-push', at: '' },
          { id: 'gone', label: 'relates_to', source: 'memory:tabs@/w/app', target: 'memory:not-here' },
        ],
      },
    },
    {
      call: { project: '/w/lib', only: 'project' },
      rows: { nodes: [mem({ slug: 'lib-rule', project: '/w/lib', imported_at: '2026-11-01T00:00:00Z', source: '/x/lib-rule.md', source_kind: 'file' }), g], edges: [] },
    },
    { call: { project: '', only: 'global' }, rows: { nodes: [g], edges: [] } },
  ];
  const graph = gm.memoryGraph(answers, projects, m.ALL);
  assert.deepEqual(graph.nodes.map((n) => n.id), [
    'memory:use-pnpm@/w/app', 'memory:use-npm@/w/app', 'memory:tabs@/w/app', 'memory:spaces@/w/app', 'memory:never-push', 'memory:lib-rule@/w/lib',
  ], 'a global memory is one node');
  const node = (slug) => graph.nodes.find((n) => n.data.slug === slug);
  assert.equal(node('use-npm').caption, 'use-npm');
  assert.deepEqual(node('use-npm').classes, ['kind-project', 'superseded']);
  assert.equal(node('never-push').data.superseded_by, 'use-pnpm', 'replaced in one project: replaced');
  assert.equal(node('never-push').data.where, 'global');
  assert.equal(node('never-push').data.context, '', 'All projects: a global memory writes as global');
  assert.equal(node('tabs').data.has_conflict, true);
  assert.equal(node('tabs').data.where, 'project app', 'every card names its project');
  assert.equal(node('lib-rule').data.where, 'project lib');
  assert.equal(node('lib-rule').data.imported, true);
  assert.deepEqual(node('lib-rule').classes, ['kind-project', 'imported']);
  assert.equal(node('lib-rule').data.context, '/w/lib');
  assert.deepEqual(graph.edges.map((e) => e.id), ['e1', 'e2', 'e3'], 'an edge to a memory not drawn is left out');
  assert.equal(graph.edges[0].data.title, 'use-pnpm replaces use-npm');
  assert.equal(graph.edges[1].data.title, 'tabs contradicts spaces');
  assert.equal(graph.edges[2].data.title, 'tabs narrows the global never-push');
  assert.equal(graph.edges[1].data.context, '/w/app');
  // One project: the global memory writes in that project's context.
  const one = gm.memoryGraph([{ call: { project: '/w/app', only: null }, rows: { nodes: [g], edges: [] } }], projects, '/w/app');
  assert.equal(one.nodes[0].data.context, '/w/app');
  assert.deepEqual(one.nodes[0].classes, ['kind-feedback', 'always']);
  assert.deepEqual(gm.memoryGraph([], projects, m.ALL), { nodes: [], edges: [] });
});

test('neighbourCalls: a project memory in its project, a global one in every project of the selection', () => {
  assert.deepEqual(gm.neighbourCalls({ scope: 'project', project: '/w/lib' }, m.ALL, projects), [{ project: '/w/lib' }]);
  assert.deepEqual(gm.neighbourCalls({ scope: 'global', project: '' }, m.ALL, projects), [{ project: '/w/app' }, { project: '/w/lib' }, { project: '' }]);
  assert.deepEqual(gm.neighbourCalls({ scope: 'global', project: '' }, '/w/app', projects), [{ project: '/w/app' }]);
  assert.deepEqual(gm.neighbourCalls({ scope: 'global', project: '' }, m.GLOBAL, projects), [{ project: '' }]);
});

test('neighbourGraph: sessions and facts with their card rows, edges to drawn memories kept, no duplicates', () => {
  const answer = {
    memory: 'memory:tabs@/w/app',
    nodes: [
      { id: 'session:s1', label: 'session', name: 'Refactor the parser', summary: 'x', project: '/w/app', started_at: '2026-11-12T09:00:00Z', harness: 'claude-code', turns: 12 },
      { id: 'fact:s1:decision:0', label: 'fact', name: 'd', summary: 'Use tabs everywhere', fact_kind: 'decision', why: 'style', at: '2026-11-12', project: '/w/app', session: 'session:s1', headline: 'Refactor the parser' },
      { id: 'fact:s1:open_thread:0', label: 'fact', summary: 'Check CI', fact_kind: 'open_thread', project: '/w/app' },
    ],
    edges: [
      { id: 'a1', label: 'about', source: 'memory:tabs@/w/app', target: 'fact:s1:decision:0' },
      { id: 'c1', label: 'contains', source: 'session:s1', target: 'fact:s1:decision:0' },
      { id: 'c1', label: 'contains', source: 'session:s1', target: 'fact:s1:decision:0' },
      { id: 'x', label: 'about', source: 'memory:other@/w/app', target: 'session:s1' },
    ],
    memories: ['memory:tabs@/w/app'],
  };
  const known = new Set(['memory:tabs@/w/app']);
  const more = gm.neighbourGraph([{ call: { project: '/w/app' }, rows: answer }, { call: { project: '' }, rows: { error: 'no memory tabs' } }], projects, known);
  assert.deepEqual(more.nodes.map((n) => [n.id, n.label]), [['session:s1', 'session'], ['fact:s1:decision:0', 'fact'], ['fact:s1:open_thread:0', 'fact']]);
  const session = more.nodes[0].data;
  assert.deepEqual([session.headline, session.harness, session.turns, session.project_name], ['Refactor the parser', 'claude-code', 12, 'app']);
  const fact = more.nodes[1];
  assert.deepEqual([fact.data.fact, fact.data.kind, fact.data.why, fact.data.project_name], ['Use tabs everywhere', 'decision', 'style', 'app']);
  assert.deepEqual(fact.classes, ['fact-decision']);
  assert.deepEqual(more.nodes[2].classes, ['fact-open-thread']);
  assert.deepEqual(more.edges.map((e) => e.id), ['a1', 'c1'], 'an edge to a memory not drawn is left out; no duplicate');
  assert.equal(more.edges[0].data.title, 'tabs is about fact:s1:decision:0');
  // A node drawn already is not added again.
  const again = gm.neighbourGraph([{ call: { project: '/w/app' }, rows: answer }], projects, new Set([...known, 'session:s1']));
  assert.deepEqual(again.nodes.map((n) => n.id), ['fact:s1:decision:0', 'fact:s1:open_thread:0']);
});

test('card buttons: memories open, pin, settle in Conflicts, forget (never an imported one); conflicts settle', () => {
  const sel = (data, label = 'memory', kind = 'node') => ({ kind, label, id: 'x', data });
  assert.deepEqual(gm.cardActions(sel({ slug: 'a', pinned: false })).map((a) => a.id), ['open', 'pin', 'forget']);
  assert.deepEqual(gm.cardActions(sel({ slug: 'a', pinned: true, has_conflict: true })).map((a) => a.id), ['open', 'unpin', 'conflicts', 'forget']);
  assert.deepEqual(gm.cardActions(sel({ slug: 'a', imported: true })).map((a) => a.id), ['open', 'pin'], 'an imported memory may be pinned, never forgotten');
  assert.deepEqual(gm.cardActions(sel({}, 'session')), []);
  assert.deepEqual(gm.cardActions(sel({}, 'fact')), []);
  assert.deepEqual(gm.cardActions(sel({ from: 'tabs', to: 'spaces' }, 'supersedes', 'edge')), []);
  const conflict = sel({ from: 'tabs', to: 'spaces', context: '/w/app' }, 'conflicts_with', 'edge');
  assert.deepEqual(gm.cardActions(conflict).map((a) => a.text), ['tabs replaces spaces', 'spaces replaces tabs', 'Both hold', 'Open Conflicts']);
  assert.deepEqual(gm.cardActions(null), []);
});

test('actionPlan: the same writes as the Memory and Conflicts tabs', () => {
  const row = { slug: 'tabs', context: '/w/app', where: 'project app', pinned: false };
  const sel = { kind: 'node', label: 'memory', id: 'memory:tabs@/w/app', data: row };
  assert.deepEqual(gm.actionPlan('open', sel), { navigate: ['memory', { slug: 'tabs', project: '/w/app' }] });
  assert.deepEqual(gm.actionPlan('conflicts', sel), { navigate: ['conflicts', {}] });
  assert.deepEqual(gm.actionPlan('pin', sel).args, { slug: 'tabs', pinned: true, order: null, project: '/w/app' });
  assert.equal(gm.actionPlan('pin', sel).call, 'pin');
  assert.deepEqual(gm.actionPlan('unpin', sel).args.pinned, false);
  const forget = gm.actionPlan('forget', sel);
  assert.equal(forget.call, 'forget');
  assert.deepEqual(forget.args, { slug: 'tabs', source: 'ui', project: '/w/app' });
  assert.match(forget.confirm, /^Forget the memory "tabs" \(project app\)\?/);
  const imported = gm.actionPlan('forget', { ...sel, data: { ...row, imported: true, file: '/x/tabs.md' } });
  assert.equal(imported.call, undefined);
  assert.match(imported.refused, /imported from \/x\/tabs\.md/);
  const pair = { kind: 'edge', label: 'conflicts_with', id: 'e2', data: { from: 'tabs', to: 'spaces', context: '/w/app' } };
  assert.deepEqual(gm.actionPlan('from-replaces', pair).args, { slug: 'tabs', decision: 'supersedes', other: 'spaces', project: '/w/app' });
  assert.deepEqual(gm.actionPlan('to-replaces', pair).args, { slug: 'spaces', decision: 'supersedes', other: 'tabs', project: '/w/app' });
  assert.deepEqual(gm.actionPlan('both', pair).args, { slug: 'tabs', decision: 'compatible', other: 'spaces', project: '/w/app' });
  assert.deepEqual(gm.actionPlan('conflicts', pair), { navigate: ['conflicts', {}] });
  assert.equal(gm.actionPlan('pin', { kind: 'node', label: 'session', data: {} }), null, 'sessions are read-only');
});

test('only a memory card has the Expand button', () => {
  assert.equal(gm.expandText({ kind: 'node', label: 'memory' }), 'Expand: sessions and facts');
  assert.equal(gm.expandText({ kind: 'node', label: 'session' }), false);
  assert.equal(gm.expandText({ kind: 'edge', label: 'supersedes' }), false);
});

test('graphSummary counts memories, replaced ones, conflicts and what was expanded', () => {
  const graph = {
    nodes: [{ label: 'memory', data: { superseded_by: 'b' } }, { label: 'memory', data: {} }, { label: 'session', data: {} }],
    edges: [{ label: 'conflicts_with' }, { label: 'supersedes' }],
  };
  assert.equal(gm.graphSummary(graph), '2 memories · 1 replaced · 1 open conflict · 1 session or fact expanded');
  assert.equal(gm.graphSummary({ nodes: [], edges: [] }), '0 memories');
});

test('the profile has the Graph tab, styles for the classes this module sets and cards for its edges', () => {
  const profile = JSON.parse(readFileSync(new URL('../profile.json', import.meta.url), 'utf8'));
  assert.ok(profile.tabs.some((t) => t.id === 'graph' && t.module === 'graph.js'));
  for (const cls of [...Object.values(gm.KIND_CLASSES), 'imported', 'always', 'superseded', 'fact-decision', 'fact-correction', 'fact-outcome', 'fact-open-thread']) {
    assert.ok(profile.graph.classes[cls], cls);
  }
  for (const label of ['supersedes', 'conflicts_with', 'narrows', 'relates_to']) {
    assert.ok(profile.graph.edges[label], label);
    assert.equal(profile.cards[label].title, 'title', label);
  }
  assert.equal(profile.graph.edges.conflicts_with.color, '--danger', 'conflicts are red');
  assert.equal(profile.graph.edges.supersedes.arrow, true, 'newer -> older');
  assert.ok(profile.cards.session.subtitle.includes('project_name'), 'a session card names its project');
});
