// The Overview tab: how many memories (by kind and scope), what every session starts with, what
// waits for a decision, the indexed sessions, and the health of the daemon and the agents'
// setup (the lines `graphersal-memory status` and `doctor` print). Read-only.

import { ref, watch, onMounted, computed, nextTick } from 'vue';
import * as model from './model.js';
import { state, init, Header, forSelection, errorText, refreshStatus, selectionName } from './shared.js';
import { getStartContext } from './api-ui.js';

export default {
  props: { api: { type: Object, required: true }, params: { type: Object, default: () => ({}) } },
  components: { Header },
  setup(props) {
    const counts = ref(null);
    const start = ref(null);
    const error = ref('');
    const busy = ref(false);
    async function load() {
      busy.value = true;
      error.value = '';
      try {
        await init();
        const answers = await forSelection(props.api, 'overview', {}, { single: true });
        if (state.selection === model.ALL) {
          const global = answers.find((a) => a.call.project === model.GLOBAL).rows;
          counts.value = model.sumOverviews(answers.filter((a) => a.call.project !== model.GLOBAL).map((a) => a.rows), global);
        } else {
          counts.value = answers[0].rows;
        }
        start.value = state.selection === model.ALL ? null : await getStartContext(state.selection);
      } catch (err) {
        error.value = errorText(err);
      } finally {
        busy.value = false;
      }
      refreshStatus();
    }
    onMounted(load);
    watch(() => [state.selection, state.version], load);
    // The top bar's notices open a section (shared.js openOverviewAt): scroll to it and
    // highlight it for a moment, then forget the request.
    async function reveal() {
      const id = model.revealId(state.reveal);
      if (!id) return;
      state.reveal = '';
      await nextTick();
      const section = document.getElementById(id);
      if (!section) return;
      section.scrollIntoView({ behavior: 'smooth', block: 'start' });
      section.classList.remove('mem-flash');
      void section.offsetWidth;
      section.classList.add('mem-flash');
      setTimeout(() => section.classList.remove('mem-flash'), 1600);
    }
    onMounted(reveal);
    watch(() => state.reveal, reveal);
    const bar = computed(() => model.budgetBar(start.value));
    const problems = computed(() => model.healthProblems(state.status));
    const indexing = computed(() => {
      const s = state.status && state.status.sessions;
      if (!s || s.ok !== true) return '';
      const report = s.report || {};
      if (s.running) return `indexing now (${report.indexed || 0} of ${report.found || 0} found)`;
      return report.active ? `${report.active} still running` : '';
    });
    const copied = ref('');
    async function copy(text) {
      try {
        await navigator.clipboard.writeText(text);
        copied.value = text;
        setTimeout(() => { if (copied.value === text) copied.value = ''; }, 2000);
      } catch (err) { /* no clipboard access: the command stays selectable */ }
    }
    return { counts, start, error, busy, bar, problems, indexing, state, model, load, selectionName, copied, copy, go: (tab) => props.api.navigate(tab) };
  },
  template: `
  <div class="mem-tab">
    <Header :api="api" />
    <p v-if="error" class="mem-error" role="alert">{{ error }}</p>
    <div v-if="counts" class="mem-tiles">
      <section class="mem-tile">
        <h3>Memories</h3>
        <p class="mem-big">{{ counts.memories }}</p>
        <p v-if="state.selection !== model.GLOBAL">{{ counts.project_memories }} in {{ state.selection === model.ALL ? 'projects' : 'this project' }} · {{ counts.global_memories }} global</p>
        <p class="muted">{{ counts.feedback }} how to work · {{ counts.user }} about you · {{ counts.project_kind }} project · {{ counts.reference }} reference</p>
        <p class="muted">{{ counts.pinned }} pinned · {{ counts.superseded }} replaced · {{ counts.imported }} imported</p>
        <button class="btn small" type="button" @click="go('memory')">Open the memories</button>
      </section>
      <section class="mem-tile">
        <h3>Always loaded</h3>
        <p class="mem-big">{{ counts.always_loaded }}</p>
        <template v-if="start">
          <p>{{ model.grouped(bar.chars) }} / {{ model.grouped(bar.budget) }} characters at every session start</p>
          <div class="mem-bar" :title="bar.percent + '%'"><div :style="{width: bar.percent + '%'}"></div></div>
          <p v-if="start.total > start.shown" class="mem-warn-text">{{ start.total - start.shown }} cut for lack of room</p>
        </template>
        <p v-else class="muted">Choose a project to see what its sessions start with.</p>
        <button class="btn small" type="button" @click="go('always')">Manage what is loaded</button>
      </section>
      <section class="mem-tile">
        <h3>Conflicts</h3>
        <p class="mem-big" :class="{bad: counts.open_conflicts > 0}">{{ counts.open_conflicts }}</p>
        <p>open {{ counts.open_conflicts === 1 ? 'conflict' : 'conflicts' }} · {{ counts.unchecked }} imported to check</p>
        <p v-if="counts.rules_off" class="muted">memory vs instruction files: off (needs an embedding model)</p>
        <p v-else-if="counts.rule_conflicts || counts.rules_unchecked" :class="{bad: counts.rule_conflicts > 0}">memory vs instruction files: {{ counts.rule_conflicts || 0 }} {{ counts.rule_conflicts === 1 ? 'conflict' : 'conflicts' }}<span v-if="counts.rules_unchecked" class="muted"> · {{ model.waitingText(counts.rules_unchecked) }}</span></p>
        <button class="btn small" type="button" @click="go('conflicts')">Decide</button>
      </section>
      <section class="mem-tile">
        <h3>Sessions</h3>
        <p class="mem-big">{{ counts.sessions }}</p>
        <p>indexed · {{ counts.facts }} facts from summaries</p>
        <p v-if="indexing" class="muted">{{ indexing }}</p>
        <button class="btn small" type="button" @click="go('sessions')">Browse</button>
      </section>
    </div>
    <p v-else-if="busy" class="muted">Loading…</p>
    <section v-if="model.updateText(state.update)" id="mem-update" class="mem-update" aria-live="polite">
      <h3>{{ model.updateText(state.update) }}</h3>
      <p>You have {{ state.update.current }}. <a v-if="state.update.release_notes" :href="state.update.release_notes" target="_blank" rel="noopener noreferrer">What's new in {{ state.update.latest }}</a></p>
      <p>Upgrade in a terminal (it shows the plan and asks first):</p>
      <div class="mem-command"><code>{{ state.update.upgrade }}</code><button class="btn small" type="button" @click="copy(state.update.upgrade)">{{ copied === state.update.upgrade ? 'Copied' : 'Copy' }}</button></div>
      <details>
        <summary>Or by hand</summary>
        <div v-for="command in state.update.manual" :key="command" class="mem-command"><code>{{ command }}</code><button class="btn small" type="button" @click="copy(command)">{{ copied === command ? 'Copied' : 'Copy' }}</button></div>
      </details>
      <p class="muted">The UI asks GitHub for the latest release at most once a day. To switch that off: {{ state.update.switch_off }}.</p>
    </section>
    <section id="mem-health" class="mem-health">
      <h3>Health</h3>
      <p>{{ model.daemonText(state.status) }}<span v-if="state.status"> · graphersal-memory {{ state.status.version }}</span><span v-if="api.info && api.info.version" class="muted"> · Graphersal engine {{ api.info.version }}</span></p>
      <ul v-if="problems.length" class="mem-problems-list">
        <li v-for="line in problems" :key="line">{{ line }}</li>
      </ul>
      <p v-else-if="state.status" class="mem-ok">No setup problems found.</p>
      <details v-if="state.status">
        <summary>What graphersal-memory status says</summary>
        <pre class="mem-pre">{{ (state.status.status || []).join('\\n') }}</pre>
      </details>
      <details v-if="state.status">
        <summary>What graphersal-memory doctor says</summary>
        <pre class="mem-pre">{{ (state.status.doctor || []).join('\\n') }}</pre>
      </details>
    </section>
  </div>`,
};
