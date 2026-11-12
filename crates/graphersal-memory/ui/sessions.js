// The Sessions tab (read-only): the newest sessions by day, search of the facts their summaries
// hold (decisions with why, corrections, outcomes, open threads) by meaning, and one session in
// full (summary, dates, branches, files; the conversation's turns on demand).

import { ref, watch, onMounted, computed } from 'vue';
import * as model from './model.js';
import { state, init, Header, forSelection, one, errorText } from './shared.js';

const PAGE = 30;

export default {
  props: { api: { type: Object, required: true }, params: { type: Object, default: () => ({}) } },
  components: { Header },
  setup(props) {
    const rows = ref([]);
    const facts = ref([]);
    const text = ref('');
    const searched = ref('');
    const opened = ref(null);
    const error = ref('');
    const busy = ref(false);
    const end = ref(false);
    async function list(more = false) {
      busy.value = true;
      error.value = '';
      try {
        await init();
        const before = more && rows.value.length ? rows.value[rows.value.length - 1].started_at : '';
        const answers = await forSelection(props.api, 'session_list', { before, limit: PAGE });
        const page = model.mergeSessions(answers, PAGE);
        rows.value = more ? rows.value.concat(page) : page;
        end.value = page.length < PAGE;
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
        const answers = await forSelection(props.api, 'sessions', { text: text.value.trim(), k: 20 });
        facts.value = model.mergeHits(answers.map((a) => ({ call: a.call, rows: a.rows.map((r) => ({ ...r, slug: r.session + '/' + r.fact })) })), 20)
          .map((f) => ({ ...f, project_name: model.projectName(f.project, state.projects) }));
        searched.value = text.value.trim();
      } catch (err) {
        error.value = errorText(err);
      } finally {
        busy.value = false;
      }
    }
    async function open(row, turns = false) {
      error.value = '';
      try {
        const id = row.id || row.session;
        const answer = await one(props.api, 'session', { id, turns, project: row.project || (state.selection === model.ALL ? '' : state.selection) });
        opened.value = { ...answer, project: row.project };
      } catch (err) {
        error.value = errorText(err);
      }
    }
    onMounted(list);
    watch(() => state.selection, () => { opened.value = null; return searched.value ? search() : list(); });
    const days = computed(() => model.sessionDays(rows.value));
    return { rows, facts, text, searched, opened, error, busy, end, days, list, search, open, state, model,
      clear: () => { text.value = ''; list(); } };
  },
  template: `
  <div class="mem-tab">
    <Header :api="api" note="Past sessions (read-only)" />
    <div class="mem-toolbar">
      <input class="mem-search" v-model="text" @keydown.enter="search" placeholder="Search past decisions, corrections, outcomes…" aria-label="Search past sessions">
      <button class="btn" type="button" @click="search">Search</button>
      <button v-if="searched" class="btn" type="button" @click="clear">Back to the sessions</button>
    </div>
    <p v-if="error" class="mem-error" role="alert">{{ error }}</p>
    <div class="mem-split">
      <div class="mem-list">
        <template v-if="searched">
          <p class="muted">{{ facts.length }} facts nearest to "{{ searched }}"</p>
          <div v-for="f in facts" :key="f.slug"><component :is="api.card('fact', f, {variant: 'compact', onOpen: (r) => open({id: r.session, project: r.project})})" /></div>
        </template>
        <template v-else>
          <p v-if="!busy && !rows.length" class="muted">No indexed session yet. Sessions are indexed when they end (or with graphersal-memory import --sessions).</p>
          <section v-for="day in days" :key="day.day">
            <h4 class="mem-day">{{ day.day }}</h4>
            <div v-for="s in day.sessions" :key="s.id"><component :is="api.card('session', s, {variant: 'compact', selected: !!opened && opened.id === s.id, onOpen: (r) => open(r)})" /></div>
          </section>
          <button v-if="!end && rows.length" class="btn" type="button" :disabled="busy" @click="list(true)">Older sessions</button>
        </template>
      </div>
      <div class="mem-open">
        <div v-if="opened" class="mem-session">
          <h3>{{ opened.headline || opened.title || opened.id }}</h3>
          <p class="muted">{{ String(opened.started_at || '').replace('T', ' ').slice(0, 16) }}<span v-if="opened.ended_at"> – {{ String(opened.ended_at).replace('T', ' ').slice(0, 16) }}</span> · {{ opened.turns }} turns<span v-if="opened.project"> · {{ model.projectName(opened.project, state.projects) }}</span></p>
          <p v-if="opened.branches && opened.branches.length"><strong>Branches:</strong> {{ opened.branches.join(', ') }}</p>
          <component :is="api.markdown(opened.body || '(no summary yet: summaries are off, or this session is not summarized)')" />
          <details v-if="opened.files_edited && opened.files_edited.length"><summary>{{ opened.files_edited.length }} files edited</summary><ul><li v-for="f in opened.files_edited" :key="f"><code>{{ f }}</code></li></ul></details>
          <p v-if="!opened.turn_list"><button class="btn small" type="button" @click="open(opened, true)">Show the conversation</button></p>
          <ol v-else class="mem-turns"><li v-for="t in opened.turn_list" :key="t.n"><span class="muted">{{ String(t.at || '').slice(11, 16) }}</span> {{ t.user }}</li></ol>
        </div>
        <p v-else class="muted">Click a session to open it.</p>
      </div>
    </div>
  </div>`,
};
