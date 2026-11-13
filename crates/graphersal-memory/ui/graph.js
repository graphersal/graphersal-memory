// The Graph tab: the memories of the selection and the decisions between them, drawn by the
// engine's graph view (`api.graph`, EXPERIMENTAL): supersedes (newer -> older), open conflicts
// (red), narrows (project -> global) and relates_to (thin grey); colour by kind, a thick border
// for the memories every session starts with, a dashed one for imported memories, faded when
// replaced (profile.json `graph`). Sessions and facts are not shown until a memory is expanded
// (double click, or Expand on its card): `memory_neighbours` adds what it is about and what is
// about it. The graph never writes; the card's buttons do what the Memory and Conflicts tabs do
// (pin, settle, forget with a confirmation, never an imported memory) or open those tabs.

import { ref, shallowRef, watch, onMounted, h } from 'vue';
import * as model from './model.js';
import * as gm from './graph-model.js';
import { state, init, Header, forSelection, one, errorText, changed } from './shared.js';

export default {
  props: { api: { type: Object, required: true }, params: { type: Object, default: () => ({}) } },
  setup(props) {
    const graph = shallowRef({ nodes: [], edges: [] });
    const error = ref('');
    const busy = ref(false);
    const loadedFor = ref(null);
    // The memories expanded (by node id): a reload after a write expands them again.
    const expanded = new Map();
    let seq = 0;

    async function neighbours(row) {
      const calls = gm.neighbourCalls(row, state.selection, state.projects);
      const answers = await Promise.all(calls.map(async (call) => ({
        call,
        rows: await one(props.api, 'memory_neighbours', { slug: row.slug, project: call.project }),
      })));
      return answers;
    }

    async function load() {
      const mine = ++seq;
      busy.value = true;
      error.value = '';
      try {
        await init();
        const answers = await forSelection(props.api, 'memory_graph', {}, { single: true });
        if (mine !== seq) return;
        let next = gm.memoryGraph(answers, state.projects, state.selection);
        if (loadedFor.value !== state.selection) expanded.clear();
        for (const [id, row] of expanded) {
          if (!next.nodes.some((n) => n.id === id)) {
            expanded.delete(id);
            continue;
          }
          const more = gm.neighbourGraph(await neighbours(row), state.projects, new Set(next.nodes.map((n) => n.id)));
          next = props.api.mergeGraph(next, more).graph;
        }
        if (mine !== seq) return;
        graph.value = next;
        loadedFor.value = state.selection;
      } catch (err) {
        error.value = errorText(err);
      } finally {
        if (mine === seq) busy.value = false;
      }
    }

    async function expand(selection) {
      if (selection.label !== 'memory') {
        props.api.toast('info', 'Only a memory expands: its sessions and facts are added.');
        return;
      }
      const row = selection.data;
      try {
        const more = gm.neighbourGraph(await neighbours(row), state.projects, new Set(graph.value.nodes.map((n) => n.id)));
        const { graph: merged, added } = props.api.mergeGraph(graph.value, more);
        graph.value = merged;
        expanded.set(selection.id, row);
        props.api.toast(added.nodes.length || added.edges.length ? 'success' : 'info', added.nodes.length
          ? `Added ${added.nodes.length} ${added.nodes.length === 1 ? 'session or fact' : 'sessions and facts'} around "${row.slug}".`
          : added.edges.length
            ? `"${row.slug}" is linked to memories already shown.`
            : `No session or fact is linked to "${row.slug}" (nothing it is about, nothing about it).`);
      } catch (err) {
        props.api.toast('error', errorText(err));
      }
    }

    async function onAction(id, selection) {
      const plan = gm.actionPlan(id, selection);
      if (!plan) return;
      if (plan.refused) {
        props.api.toast('warn', plan.refused);
        return;
      }
      if (plan.navigate) {
        props.api.navigate(plan.navigate[0], plan.navigate[1]);
        return;
      }
      if (plan.confirm && !window.confirm(plan.confirm)) return;
      await one(props.api, plan.call, plan.args);
      props.api.toast('success', plan.done);
      changed();
    }

    onMounted(load);
    watch(() => [state.selection, state.version], load);

    return () => h('div', { class: 'mem-tab mem-graph-tab' }, [
      h(Header, { api: props.api, note: 'Memories and the decisions between them' }),
      error.value ? h('p', { class: 'mem-error', role: 'alert' }, error.value) : null,
      h('p', { class: 'muted mem-graph-help' }, [
        busy.value ? 'Loading… ' : `${gm.graphSummary(graph.value)}. `,
        'Click a memory for its card; double click (or Expand) adds its sessions and facts. The graph only reads: the card\'s buttons write, as in the other tabs.',
      ]),
      props.api.graph(graph.value, {
        onExpand: expand,
        card: { actions: gm.cardActions, onAction, expand: gm.expandText },
        height: '64vh',
        emptyText: busy.value ? 'Loading…' : (state.selection === model.GLOBAL ? 'No global memory yet.' : 'No memory yet.'),
      }),
    ]);
  },
};
