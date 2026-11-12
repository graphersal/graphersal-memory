// What the memory profile's tabs share (one module instance for every tab): the project
// selection and the project list, the header (project switcher, conflicts, daemon), the calls
// of the memory functions for a selection ("All projects" = once per project plus the global
// memories once), and the memory form with the check of the nearest memories after a save.
// Plain words everywhere; no query language.

import { reactive, ref, computed, h } from 'vue';
import * as model from './model.js';
import { getProjects, getStatus } from './api-ui.js';

const STORE_KEY = 'graphersal-memory.ui.project';

/** The state every tab sees. */
export const state = reactive({
  projects: [],
  selection: model.ALL,
  loaded: false,
  status: null,
  statusError: '',
  /** Bumped after every write: tabs reload what they show. */
  version: 0,
});

let loading = null;

/** Loads the projects (once) and the stored selection. */
export function init() {
  if (!loading) {
    loading = (async () => {
      try {
        state.projects = await getProjects();
      } catch (err) {
        state.projects = [];
        state.statusError = err.message;
      }
      let stored = null;
      try { stored = window.localStorage.getItem(STORE_KEY); } catch (e) { /* storage blocked */ }
      state.selection = model.validSelection(stored === null ? model.ALL : stored, state.projects);
      state.loaded = true;
      refreshStatus();
    })();
  }
  return loading;
}

/** Selects a project, "" (global) or ALL. */
export function select(selection) {
  state.selection = model.validSelection(selection, state.projects);
  try { window.localStorage.setItem(STORE_KEY, state.selection); } catch (e) { /* storage blocked */ }
}

/** Marks the data as changed (after a write). */
export function changed() {
  state.version += 1;
  refreshStatus();
}

/** Reads the daemon status again. */
export async function refreshStatus() {
  try {
    state.status = await getStatus();
    state.statusError = '';
  } catch (err) {
    state.statusError = err.message;
  }
}

/** The name of the current selection. */
export const selectionName = () => model.selectionName(state.selection, state.projects);

/** The results of a memory function; a map with `error` is thrown as an Error. */
export async function run(api, name, params) {
  const results = await api.query(name, params);
  const first = results.length === 1 ? results[0] : null;
  if (first && typeof first === 'object' && !Array.isArray(first) && typeof first.error === 'string') {
    throw new Error(first.error);
  }
  return results;
}

/** The single map a memory function answers. */
export async function one(api, name, params) {
  const results = await run(api, name, params);
  return results[0] || {};
}

/**
 * One read for the current selection: `[{call, rows}]` per call of `model.callsFor` (rows: the
 * single map with `single`, else the list).
 */
export async function forSelection(api, name, params, { single = false, selection = state.selection } = {}) {
  const calls = model.callsFor(selection, state.projects);
  return Promise.all(calls.map(async (call) => {
    const results = await run(api, name, { ...params, project: call.project });
    return { call, rows: single ? (results[0] || {}) : results };
  }));
}

/** A short error text of a refused call. */
export function errorText(err) {
  if (!err) return '';
  return err.help ? `${err.message} ${err.help}` : (err.message || String(err));
}

/** The header of every tab: the project switcher, the open conflicts and the daemon. */
export const Header = {
  props: { api: { type: Object, required: true }, note: { type: String, default: '' }, allowAll: { type: Boolean, default: true } },
  setup() {
    init();
    const problems = computed(() => model.healthProblems(state.status));
    return { state, model, problems, choose: select };
  },
  template: `
  <div class="mem-header">
    <label class="mem-switch">
      <span class="muted">Project</span>
      <select :value="state.selection" @change="choose($event.target.value)" aria-label="Project">
        <option v-if="allowAll" :value="model.ALL">All projects</option>
        <option :value="model.GLOBAL">Global (every project)</option>
        <option v-for="p in state.projects" :key="p.id" :value="p.id" :title="p.paths && p.paths[0]">{{ p.name }}</option>
      </select>
    </label>
    <span class="mem-daemon" :class="{bad: !state.status || !state.status.daemon}">{{ model.daemonText(state.status) }}</span>
    <span v-if="problems.length" class="mem-problems" :title="problems.join('\\n')">{{ problems.length }} setup {{ problems.length === 1 ? 'problem' : 'problems' }} (Overview)</span>
    <span v-if="note" class="muted mem-note">{{ note }}</span>
    <span v-if="state.statusError" class="mem-error">{{ state.statusError }}</span>
  </div>`,
};

/**
 * After a save: the nearest memories `remember` found, each with the decisions it allows
 * (settle). Nothing changes until a button is clicked.
 */
export const NearestCheck = {
  props: { api: { type: Object, required: true }, saved: { type: Object, required: true } },
  emits: ['done'],
  setup(props, { emit }) {
    const decided = reactive({});
    const busy = ref('');
    async function decide(near, choice) {
      busy.value = near.slug;
      try {
        await one(props.api, 'settle', { slug: props.saved.slug, decision: choice.id, other: near.slug, project: props.saved.project });
        decided[near.slug] = choice.text;
        props.api.toast('success', `Recorded: ${choice.text.toLowerCase()}.`);
        changed();
      } catch (err) {
        props.api.toast('error', errorText(err));
      } finally {
        busy.value = '';
      }
    }
    const choices = (near) => model.nearestChoices(props.saved, near);
    return { decided, busy, decide, choices, model, emit };
  },
  template: `
  <div class="mem-check">
    <p><strong>Saved.</strong> These memories are close to "{{ saved.slug }}". Most are only on a similar topic; say how each relates, or leave it.</p>
    <div v-for="near in saved.nearest" :key="near.slug + near.scope" class="mem-near">
      <div class="mem-near-text">
        <strong>{{ near.slug }}</strong>
        <span class="muted"> · {{ near.kind }} · {{ near.scope }} · {{ model.sourceText(near) }}</span>
        <div>{{ model.unmarked(near.summary) }}</div>
      </div>
      <div class="mem-near-actions" v-if="!decided[near.slug]">
        <button v-for="c in choices(near)" :key="c.id" class="btn small" type="button" :title="c.title" :disabled="!!busy" @click="decide(near, c)">{{ c.text }}</button>
      </div>
      <div v-else class="mem-ok">{{ decided[near.slug] }}</div>
    </div>
    <p><button class="btn" type="button" @click="emit('done')">Close</button></p>
  </div>`,
};

/**
 * The memory form: create (where, kind, name, ...) or edit (the memory as it was opened; before
 * saving it is read again and a memory that changed meanwhile is not overwritten).
 */
export const MemoryForm = {
  props: {
    api: { type: Object, required: true },
    memory: { type: Object, default: null },
    project: { type: String, default: '' },
  },
  emits: ['done', 'cancel'],
  setup(props, { emit }) {
    const creating = !props.memory;
    const form = reactive(creating
      ? { slug: '', kind: 'feedback', scope: props.project && props.project !== model.ALL ? 'project' : 'global', title: '', summary: '', hook: '', body: '' }
      : model.formOf(props.memory));
    const target = ref(creating ? (props.project === model.ALL ? model.GLOBAL : props.project) : props.project);
    const problems = ref([]);
    const busy = ref(false);
    const saved = ref(null);
    const meanwhile = ref(false);
    const whereText = computed(() => (form.scope === 'global' || !target.value
      ? 'Global: every project'
      : `Project ${model.projectName(target.value, state.projects)}`));
    async function save() {
      problems.value = model.formProblems(form, { creating });
      if (problems.value.length) return;
      busy.value = true;
      try {
        const project = target.value === model.ALL ? model.GLOBAL : target.value;
        if (creating) {
          form.scope = project ? 'project' : 'global';
          const existing = await props.api.query('memory', { slug: form.slug, project });
          const found = existing[0];
          if (found && !found.error && found.scope === form.scope) {
            problems.value = [`A memory named "${form.slug}" exists already (${found.scope}). Choose another name, or edit that one.`];
            return;
          }
        } else {
          const now = (await props.api.query('memory', { slug: props.memory.slug, project }))[0];
          if (model.changedMeanwhile(props.memory, now)) {
            meanwhile.value = true;
            return;
          }
        }
        const answer = await one(props.api, 'remember', model.rememberArgs(form, project, props.memory));
        changed();
        props.api.toast('success', `Saved "${form.slug}".`);
        if (answer.nearest && answer.nearest.length) {
          saved.value = { ...answer, project };
        } else {
          emit('done', answer);
        }
      } catch (err) {
        problems.value = [errorText(err)];
      } finally {
        busy.value = false;
      }
    }
    return { form, target, problems, busy, save, saved, meanwhile, creating, model, state, whereText, emit };
  },
  components: { NearestCheck },
  template: `
  <div class="mem-form">
    <NearestCheck v-if="saved" :api="api" :saved="saved" @done="emit('done', saved)" />
    <template v-else>
      <h3>{{ creating ? 'New memory' : 'Edit "' + form.slug + '"' }}</h3>
      <div v-if="meanwhile" class="mem-warn" role="alert">
        This memory changed since you opened it (an agent or another tab wrote it). Nothing was saved.
        <button class="btn small" type="button" @click="emit('cancel', 'reload')">Open it again</button>
      </div>
      <div class="mem-grid">
        <label>Where</label>
        <div v-if="creating">
          <select v-model="target">
            <option :value="model.GLOBAL">Global: every project</option>
            <option v-for="p in state.projects" :key="p.id" :value="p.id">Project {{ p.name }}</option>
          </select>
        </div>
        <div v-else class="mem-fixed">{{ whereText }}</div>
        <label>Kind</label>
        <select v-model="form.kind"><option v-for="k in model.KINDS" :key="k.id" :value="k.id">{{ k.text }}</option></select>
        <label v-if="creating">Name</label>
        <input v-if="creating" v-model.trim="form.slug" placeholder="lowercase-words-with-dashes" spellcheck="false">
        <label>Title</label>
        <input v-model="form.title" placeholder="optional: a short title">
        <label>Summary</label>
        <input v-model="form.summary" placeholder="one line: what to remember">
        <label>Start line</label>
        <input v-model="form.hook" placeholder="optional: the line every session start shows (else the summary)">
        <label>Text</label>
        <textarea v-model="form.body" rows="8" placeholder="the memory in full (Markdown); say why and how to apply it"></textarea>
      </div>
      <ul v-if="problems.length" class="mem-problems-list"><li v-for="p in problems" :key="p">{{ p }}</li></ul>
      <p class="mem-actions">
        <button class="btn primary" type="button" :disabled="busy" @click="save">{{ busy ? 'Saving…' : 'Save' }}</button>
        <button class="btn" type="button" :disabled="busy" @click="emit('cancel')">Cancel</button>
      </p>
    </template>
  </div>`,
};

/** A VNode of `api.card` for templates (`<component :is="card(...)" />`). */
export function cardNode(api, label, row, options) {
  return api.card(label, row, options);
}

/** A little helper for render functions. */
export { h };
