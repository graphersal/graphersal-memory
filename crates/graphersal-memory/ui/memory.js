// The Memory tab: the memories of the selection (filters: kind, scope, source, replaced ones),
// search by meaning (what the agent gets from memories(text)), a memory in full, and create /
// edit / pin / forget. Imported memories are read-only here: their file is the one to edit.

import { ref, reactive, watch, onMounted, computed } from 'vue';
import * as model from './model.js';
import { state, init, Header, MemoryForm, forSelection, one, errorText, changed } from './shared.js';

const PAGE = 100;

export default {
  props: { api: { type: Object, required: true }, params: { type: Object, default: () => ({}) } },
  components: { Header, MemoryForm },
  setup(props) {
    const filters = reactive({ kind: '', scope: '', source: '', superseded: false });
    const text = ref('');
    const searched = ref('');
    const rows = ref([]);
    const total = ref(0);
    const offset = ref(0);
    const opened = ref(null);
    const openedRaw = ref(null);
    const editing = ref(null);
    const creating = ref(false);
    const error = ref('');
    const busy = ref(false);

    async function list(more = false) {
      busy.value = true;
      error.value = '';
      try {
        await init();
        const all = state.selection === model.ALL;
        const start = more ? offset.value + PAGE : 0;
        const answers = await forSelection(props.api, 'memory_list', {
          kind: filters.kind, scope: filters.scope, source_kind: filters.source,
          superseded: filters.superseded, offset: all ? 0 : start, limit: all ? 500 : PAGE,
        }, { single: true });
        const merged = model.mergeRows(answers.map((a) => ({ call: a.call, rows: a.rows.rows || [] })))
          .map((row) => model.memoryRow(row, state.projects, state.selection));
        rows.value = more ? rows.value.concat(merged) : merged;
        total.value = all ? merged.length : (answers[0].rows.total || 0);
        offset.value = start;
        searched.value = '';
      } catch (err) {
        error.value = errorText(err);
      } finally {
        busy.value = false;
      }
    }

    async function search() {
      if (!text.value.trim()) return list();
      busy.value = true;
      error.value = '';
      try {
        const answers = await forSelection(props.api, 'memories', { text: text.value.trim(), k: 20, kind: filters.kind });
        rows.value = model.mergeHits(answers, 20).map((row) => model.memoryRow(row, state.projects, state.selection));
        total.value = rows.value.length;
        searched.value = text.value.trim();
      } catch (err) {
        error.value = errorText(err);
      } finally {
        busy.value = false;
      }
    }

    async function open(row) {
      error.value = '';
      try {
        const answer = await one(props.api, 'memory', { slug: row.slug, project: row.context });
        openedRaw.value = answer;
        opened.value = model.memoryRow(answer, state.projects, row.context);
        editing.value = null;
        creating.value = false;
      } catch (err) {
        error.value = errorText(err);
      }
    }

    async function onAction(id, row) {
      if (id === 'edit') {
        editing.value = { memory: openedRaw.value, project: row.context };
        return;
      }
      if (id === 'pin' || id === 'unpin') {
        await one(props.api, 'pin', { slug: row.slug, pinned: id === 'pin', order: null, project: row.context });
        props.api.toast('success', id === 'pin' ? `Pinned "${row.slug}": every session starts with it.` : `Unpinned "${row.slug}".`);
        changed();
        await open(row);
        return;
      }
      if (id === 'forget') {
        if (!window.confirm(`Forget the memory "${row.slug}" (${row.where})? Agents will no longer find it; this cannot be undone here.`)) return;
        await one(props.api, 'forget', { slug: row.slug, source: 'ui', project: row.context });
        props.api.toast('success', `Forgot "${row.slug}".`);
        opened.value = null;
        changed();
      }
    }

    function formDone() {
      const slug = editing.value && editing.value.memory.slug;
      const context = editing.value && editing.value.project;
      editing.value = null;
      creating.value = false;
      if (slug) open({ slug, context });
    }
    function formCancel(reason) {
      const current = opened.value;
      editing.value = null;
      creating.value = false;
      if (reason === 'reload' && current) open(current);
    }

    onMounted(() => { if (props.params.slug) open({ slug: props.params.slug, context: props.params.project || '' }); list(); });
    // Another tab's "Open in Memory" while this tab stays mounted: open that memory too.
    watch(() => [props.params.slug, props.params.project], ([slug, project]) => {
      if (slug) open({ slug, context: project || '' });
    });
    watch(() => [state.selection, state.version], () => (searched.value ? search() : list()));
    watch(() => [filters.kind, filters.scope, filters.source, filters.superseded], () => list());
    const more = computed(() => !searched.value && state.selection !== model.ALL && rows.value.length < total.value);
    const readOnlyNote = computed(() => (opened.value && opened.value.imported
      ? `Imported from ${opened.value.file}: edit that file instead (pin, order and conflict decisions work here).`
      : ''));
    return {
      filters, text, rows, total, opened, editing, creating, error, busy, searched, more, readOnlyNote,
      list, search, open, onAction, formDone, formCancel, state, model,
      clear: () => { text.value = ''; list(); },
    };
  },
  template: `
  <div class="mem-tab">
    <Header :api="api" />
    <div class="mem-toolbar">
      <input class="mem-search" v-model="text" @keydown.enter="search" placeholder="Search by meaning (what your agent finds)…" aria-label="Search by meaning">
      <button class="btn" type="button" @click="search">Search</button>
      <button v-if="searched" class="btn" type="button" @click="clear">Show all</button>
      <select v-model="filters.kind" aria-label="Kind"><option value="">every kind</option><option v-for="k in model.KINDS" :key="k.id" :value="k.id">{{ k.text }}</option></select>
      <select v-model="filters.scope" aria-label="Scope" :disabled="!!searched"><option value="">project and global</option><option value="project">project only</option><option value="global">global only</option></select>
      <select v-model="filters.source" aria-label="Source" :disabled="!!searched"><option value="">every source</option><option v-for="s in model.SOURCES" :key="s.id" :value="s.id">{{ s.text }}</option></select>
      <label class="mem-check-label"><input type="checkbox" v-model="filters.superseded" :disabled="!!searched"> show replaced</label>
      <button class="btn primary" type="button" @click="creating = true; editing = null">+ New</button>
    </div>
    <p v-if="error" class="mem-error" role="alert">{{ error }}</p>
    <div class="mem-split">
      <div class="mem-list">
        <p class="muted">{{ searched ? rows.length + ' nearest to "' + searched + '"' : total + (total === 1 ? ' memory' : ' memories') }}<span v-if="busy"> · loading…</span></p>
        <div v-for="row in rows" :key="row.context + '/' + row.scope + '/' + row.slug">
          <component :is="api.card('memory', row, {variant: 'compact', selected: !!opened && opened.slug === row.slug && opened.scope === row.scope, onOpen: open})" />
        </div>
        <p v-if="!busy && !rows.length" class="muted">No memory matches.</p>
        <button v-if="more" class="btn" type="button" @click="list(true)">More</button>
      </div>
      <div class="mem-open">
        <MemoryForm v-if="creating" :api="api" :project="state.selection" @done="formDone" @cancel="formCancel" />
        <MemoryForm v-else-if="editing" :key="editing.memory.slug" :api="api" :memory="editing.memory" :project="editing.project" @done="formDone" @cancel="formCancel" />
        <template v-else-if="opened">
          <p v-if="readOnlyNote" class="mem-note-box">{{ readOnlyNote }}</p>
          <component :is="api.card('memory', opened, {onAction})" />
        </template>
        <p v-else class="muted">Click a memory to open it.</p>
      </div>
    </div>
  </div>`,
};
