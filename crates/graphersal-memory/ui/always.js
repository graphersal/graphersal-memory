// The Always loaded tab: what every session of a project starts with, in delivery order and
// grouped as the agent sees it, the budget bar, the literal preview, the memories cut for lack
// of room, pin/unpin, move up/down (set_order renumbers the group), edit a memory's start line,
// and the budget. Warnings: the budget too small for the pinned memories, a pinned memory in an
// open conflict, a pinned memory that was replaced.

import { ref, reactive, watch, onMounted, computed } from 'vue';
import * as model from './model.js';
import { state, init, Header, one, run, errorText, changed, selectionName } from './shared.js';
import { getStartContext, getSettings, setBudget } from './api-ui.js';

export default {
  props: { api: { type: Object, required: true }, params: { type: Object, default: () => ({}) } },
  components: { Header },
  setup(props) {
    const context = ref(null);
    const rows = ref([]);
    const settings = ref(null);
    const budget = ref(0);
    const error = ref('');
    const busy = ref('');
    const line = reactive({ slug: '', scope: '', text: '', opened: null, problem: '' });

    async function load() {
      error.value = '';
      await init();
      if (state.selection === model.ALL) {
        context.value = null;
        return;
      }
      try {
        const [ctx, list, current] = await Promise.all([
          getStartContext(state.selection),
          one(props.api, 'memory_list', { kind: '', scope: '', source_kind: '', superseded: true, offset: 0, limit: 500, project: state.selection }),
          getSettings(),
        ]);
        context.value = ctx;
        rows.value = (list.rows || []).map((row) => model.memoryRow(row, state.projects, state.selection));
        settings.value = current;
        budget.value = current.start_context_chars;
      } catch (err) {
        error.value = errorText(err);
      }
    }
    onMounted(load);
    watch(() => [state.selection, state.version], load);

    const entries = computed(() => (context.value && context.value.entries) || []);
    const groups = computed(() => model.sections(entries.value));
    const cut = computed(() => entries.value.filter((e) => !e.shown));
    const bar = computed(() => model.budgetBar(context.value));
    const warnings = computed(() => model.warnings(entries.value, rows.value));
    const rowOf = (entry) => rows.value.find((r) => r.slug === entry.slug && r.scope === entry.scope) || {};

    async function act(label, work) {
      busy.value = label;
      try {
        await work();
        changed();
      } catch (err) {
        props.api.toast('error', errorText(err));
      } finally {
        busy.value = '';
      }
    }
    const move = (entry, delta) => act(entry.slug, async () => {
      const slugs = model.moved(entries.value, entry.slug, delta);
      if (slugs) await one(props.api, 'set_order', { slugs, project: state.selection });
    });
    const togglePin = (entry) => act(entry.slug, async () => {
      await one(props.api, 'pin', { slug: entry.slug, pinned: !entry.pinned, order: null, project: state.selection });
      props.api.toast('success', entry.pinned ? `Unpinned "${entry.slug}".` : `Pinned "${entry.slug}".`);
    });
    const unmark = (entry) => act(entry.slug, async () => {
      await one(props.api, 'pin', { slug: entry.slug, pinned: false, order: null, project: state.selection });
      props.api.toast('success', `"${entry.slug}" now follows its section's order (still always loaded).`);
    });
    async function editLine(entry) {
      line.problem = '';
      try {
        const memory = await one(props.api, 'memory', { slug: entry.slug, project: state.selection });
        line.slug = entry.slug;
        line.scope = entry.scope;
        line.opened = memory;
        line.text = model.formOf(memory).hook || model.unmarked(memory.summary);
      } catch (err) {
        props.api.toast('error', errorText(err));
      }
    }
    const saveLine = () => act(line.slug, async () => {
      const now = (await run(props.api, 'memory', { slug: line.slug, project: state.selection }))[0];
      if (model.changedMeanwhile(line.opened, now)) {
        line.problem = 'This memory changed since you opened it (an agent or another tab wrote it). Nothing was saved; open it again.';
        throw new Error(line.problem);
      }
      const form = { ...model.formOf(line.opened), hook: line.text };
      await one(props.api, 'remember', model.rememberArgs(form, state.selection, line.opened));
      props.api.toast('success', `Saved the start line of "${line.slug}".`);
      line.slug = '';
    });
    const saveBudget = () => act('budget', async () => {
      const n = Number(budget.value);
      if (!Number.isInteger(n) || n < 0) throw new Error('The budget is a whole number of characters (0 = nothing is loaded).');
      settings.value = await setBudget(n);
      props.api.toast('success', `Every session start may now use ${model.grouped(n)} characters.`);
    });
    return {
      context, groups, cut, bar, warnings, settings, budget, error, busy, line, state, model,
      move, togglePin, unmark, editLine, saveLine, saveBudget, rowOf, selectionName, entries,
      isFirst: (entry) => model.moved(entries.value, entry.slug, -1) === null,
      isLast: (entry) => model.moved(entries.value, entry.slug, 1) === null,
    };
  },
  template: `
  <div class="mem-tab">
    <Header :api="api" note="What every session starts with" />
    <p v-if="error" class="mem-error" role="alert">{{ error }}</p>
    <p v-if="state.selection === model.ALL" class="mem-note-box">Every project starts with its own memories plus the global ones: choose a project (or Global) above.</p>
    <template v-else-if="context">
      <section class="mem-budget">
        <div class="mem-budget-line">
          <strong>Budget</strong>
          <div class="mem-bar wide" :title="bar.percent + '%'"><div :style="{width: bar.percent + '%'}"></div></div>
          <span>{{ model.grouped(bar.chars) }} / {{ model.grouped(bar.budget) }} characters</span>
        </div>
        <div class="mem-budget-line">
          <label>Characters every session start may use <input type="number" min="0" step="500" v-model.number="budget" class="mem-num"></label>
          <button class="btn small" type="button" :disabled="busy === 'budget' || !settings || budget === settings.start_context_chars" @click="saveBudget">Change</button>
          <span class="muted" v-if="settings">default {{ model.grouped(settings.default_start_context_chars) }}</span>
        </div>
      </section>
      <ul v-if="warnings.length" class="mem-warn-list"><li v-for="w in warnings" :key="w">⚠ {{ w }}</li></ul>
      <p v-if="!entries.length" class="muted">Nothing is loaded at the start yet: pin a memory, or add a "how to work" or "about you" memory.</p>
      <section v-for="group in groups" :key="group.id" class="mem-group">
        <h3>{{ group.title }}</h3>
        <p v-if="group.note" class="muted mem-section-note">{{ group.note }}</p>
        <div v-for="entry in group.entries" :key="entry.scope + entry.slug" class="mem-entry" :class="{cut: !entry.shown}">
          <span class="mem-moves">
            <button class="icon" type="button" :disabled="!!busy || isFirst(entry)" :aria-label="'Move ' + entry.slug + ' up'" title="Move up" @click="move(entry, -1)">↑</button>
            <button class="icon" type="button" :disabled="!!busy || isLast(entry)" :aria-label="'Move ' + entry.slug + ' down'" title="Move down" @click="move(entry, 1)">↓</button>
          </span>
          <label v-if="model.pinControls(group, entry).checkbox" class="mem-pin" :title="entry.pinned ? 'Unpin' : 'Pin: always load it'"><input type="checkbox" :checked="entry.pinned" :disabled="!!busy" @change="togglePin(entry)"> pinned</label>
          <span v-else-if="model.pinControls(group, entry).keptFirst" class="mem-pin">
            <span class="mem-kept-badge" :title="model.KEPT_FIRST_TITLE">kept first</span>
            <button class="icon" type="button" :disabled="!!busy" :aria-label="'Remove the kept-first mark of ' + entry.slug" title="Remove the mark (it stays always loaded)" @click="unmark(entry)">×</button>
          </span>
          <div class="mem-entry-text">
            <template v-if="line.slug === entry.slug && line.scope === entry.scope">
              <input v-model="line.text" class="mem-line-input" :aria-label="'Start line of ' + entry.slug">
              <button class="btn small primary" type="button" :disabled="!!busy" @click="saveLine">Save</button>
              <button class="btn small" type="button" @click="line.slug = ''">Cancel</button>
              <div v-if="line.problem" class="mem-error">{{ line.problem }}</div>
            </template>
            <template v-else>
              <strong>{{ entry.title }}</strong> — {{ entry.hook }}
              <span class="muted"> · {{ entry.scope === 'global' ? 'global' : 'project' }} · {{ entry.slug }}</span>
              <span v-for="label in model.reachLabels(entry)" :key="label.id" class="mem-cut-badge" :title="label.title">{{ label.text }}</span>
            </template>
          </div>
          <button v-if="!(line.slug === entry.slug && line.scope === entry.scope)" class="btn small" type="button" :title="model.importNote(rowOf(entry))" @click="editLine(entry)">Edit line</button>
        </div>
      </section>
      <p v-if="groups.some((g) => g.entries.some((e) => e.scope === 'global'))" class="muted">Moving a global memory changes its place in every project.</p>
      <section v-if="cut.length" class="mem-group">
        <h3>Not shown (over budget): {{ cut.length }}</h3>
        <p class="muted">{{ cut.map((e) => e.slug).join(', ') }} — the agent is told they exist and can search them.</p>
      </section>
      <details class="mem-preview" open>
        <summary>Preview: exactly what the agent gets</summary>
        <pre class="mem-pre">{{ context.text || '(nothing: the start context is empty)' }}</pre>
      </details>
    </template>
  </div>`,
};
