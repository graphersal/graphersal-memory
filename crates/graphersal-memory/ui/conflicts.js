// The Conflicts tab: open conflicts side by side with the decisions (settle: which one replaces
// the other, both hold; or forget one, never an imported one), and the imported memories not yet
// checked against their nearest memories, in a collapsed section, checked one by one (no bulk
// button: user decision 9a). Then "Memory vs instruction files": a memory beside the line of an
// instruction file the harness loads (CLAUDE.md, AGENTS.md, rules files) that the agent found to
// contradict it: both hold, the memory is outdated (forget it; never an imported one), or the
// file is outdated (recorded as a conflict; the user edits the file, nothing here writes it).
// Pairs the agent has not checked yet are its work, not the user's (user decision 2026-11-14):
// one quiet line with their count, the pairs in a collapsed disclosure without buttons.

import { ref, reactive, watch, onMounted, computed } from 'vue';
import * as model from './model.js';
import { state, init, Header, forSelection, one, errorText, changed } from './shared.js';

export default {
  props: { api: { type: Object, required: true }, params: { type: Object, default: () => ({}) } },
  components: { Header },
  setup(props) {
    const data = ref(null);
    const error = ref('');
    const busy = ref('');
    const later = reactive(new Set());
    const expanded = reactive({});
    const checking = ref(false);
    const checked = reactive({});
    async function load() {
      error.value = '';
      try {
        await init();
        const answers = await forSelection(props.api, 'conflicts', { k: 20 }, { single: true });
        data.value = model.mergeConflicts(answers);
      } catch (err) {
        error.value = errorText(err);
      }
    }
    onMounted(load);
    watch(() => [state.selection, state.version], load);
    const keyOf = (pair) => `${pair.a.scope}/${pair.a.slug}|${pair.b.scope}/${pair.b.slug}`;
    const open = computed(() => (data.value ? data.value.open.filter((p) => !later.has(keyOf(p))) : []));
    const current = computed(() => (data.value && data.value.unchecked.length ? data.value.unchecked[0] : null));

    async function act(label, work, done) {
      busy.value = label;
      try {
        await work();
        if (done) props.api.toast('success', done);
        changed();
      } catch (err) {
        props.api.toast('error', errorText(err));
      } finally {
        busy.value = '';
      }
    }
    const settle = (pair, slug, decision, other, done) => act(keyOf(pair), () => one(props.api, 'settle', { slug, decision, other, project: pair.project }), done);
    function forget(pair, side) {
      const memory = pair[side];
      if (model.isImported(memory)) return;
      if (!window.confirm(`Forget the memory "${memory.slug}" (${memory.scope})? The other one stays; this cannot be undone here.`)) return;
      act(keyOf(pair), () => one(props.api, 'forget', { slug: memory.slug, source: 'ui', project: pair.project }), `Forgot "${memory.slug}".`);
    }
    const choices = (memory, near) => model.nearestChoices(memory, near);
    function decide(memory, near, choice) {
      act(memory.slug, async () => {
        await one(props.api, 'settle', { slug: memory.slug, decision: choice.id, other: near.slug, project: memory.project });
        checked[`${memory.slug}/${near.slug}`] = choice.text;
      });
    }
    const done = (memory) => act(memory.slug, () => one(props.api, 'settle', { slug: memory.slug, decision: 'compatible', other: '', project: memory.project }), `"${memory.slug}" is checked.`);
    const rulePairs = computed(() => (data.value ? data.value.rules.filter((p) => p.status === 'conflict' && !later.has(model.rulePairKey(p))) : []));
    const waitingPairs = computed(() => (data.value ? data.value.rules.filter((p) => p.status !== 'conflict') : []));
    const fileNotes = reactive({});
    const settleRule = (pair, args, done) => act(model.rulePairKey(pair), () => one(props.api, 'settle', args), done);
    function forgetMemory(pair) {
      const actions = model.ruleActions(pair);
      if (!actions.canForget) return;
      if (!window.confirm(`Forget the memory "${pair.memory.slug}" (${pair.memory.scope})? The line of ${model.fileLine(pair.rule)} stays; this cannot be undone here.`)) return;
      act(model.rulePairKey(pair), () => one(props.api, 'forget', { slug: pair.memory.slug, source: 'ui', project: pair.project }), `Forgot "${pair.memory.slug}".`);
    }
    function fileOutdated(pair) {
      const key = model.rulePairKey(pair);
      fileNotes[key] = true;
      if (pair.status !== 'conflict') settleRule(pair, model.ruleActions(pair).conflict, 'Recorded: the file is outdated; it stays listed until you edit that line.');
    }
    const edit = (memory, project) => props.api.navigate('memory', { slug: memory.slug, project: memory.scope === 'global' ? (project || '') : project });
    return {
      data, error, busy, open, current, expanded, checking, checked, later, keyOf, state, model,
      settle, forget, choices, decide, done, edit, load,
      rulePairs, waitingPairs, fileNotes, settleRule, forgetMemory, fileOutdated,
    };
  },
  template: `
  <div class="mem-tab">
    <Header :api="api" note="Memories that contradict each other" />
    <p v-if="error" class="mem-error" role="alert">{{ error }}</p>
    <template v-if="data">
      <h3>{{ open.length }} open {{ open.length === 1 ? 'conflict' : 'conflicts' }}</h3>
      <p v-if="!open.length" class="mem-ok">Nothing contradicts.</p>
      <section v-for="pair in open" :key="keyOf(pair)" class="mem-conflict">
        <p class="muted">since {{ pair.since || 'unknown' }}<span v-if="state.selection === model.ALL"> · {{ model.projectName(pair.project, state.projects) }}</span></p>
        <div class="mem-sides">
          <div v-for="side in ['a', 'b']" :key="side" class="mem-side">
            <h4>{{ pair[side].slug }} <span class="muted">({{ model.sourceText(pair[side]) }})</span></h4>
            <p class="muted">{{ pair[side].kind }} · {{ pair[side].scope }} · {{ String(pair[side].updated_at || '').slice(0, 10) }}</p>
            <p>{{ model.unmarked(pair[side].summary) }}</p>
            <button class="btn small" type="button" @click="edit(pair[side], pair.project)">Open</button>
          </div>
        </div>
        <div class="mem-conflict-actions">
          <button class="btn" type="button" :disabled="!!busy" @click="settle(pair, pair.b.slug, 'supersedes', pair.a.slug, 'Recorded: ' + pair.b.slug + ' replaces ' + pair.a.slug + '.')">Right replaces left</button>
          <button class="btn" type="button" :disabled="!!busy" @click="settle(pair, pair.a.slug, 'supersedes', pair.b.slug, 'Recorded: ' + pair.a.slug + ' replaces ' + pair.b.slug + '.')">Left replaces right</button>
          <button class="btn" type="button" :disabled="!!busy" @click="expanded[keyOf(pair)] = expanded[keyOf(pair)] === 'both' ? '' : 'both'">Both true in different contexts…</button>
          <button class="btn" type="button" :disabled="!!busy" @click="expanded[keyOf(pair)] = expanded[keyOf(pair)] === 'delete' ? '' : 'delete'">Delete one…</button>
          <button class="btn" type="button" @click="later.add(keyOf(pair))">Later</button>
        </div>
        <div v-if="expanded[keyOf(pair)] === 'both'" class="mem-note-box">
          Both hold. Say in one of them where it applies (open it and edit its text), then record it:
          <button class="btn small primary" type="button" :disabled="!!busy" @click="settle(pair, pair.a.slug, 'compatible', pair.b.slug, 'Recorded: both hold.')">Both hold</button>
        </div>
        <div v-if="expanded[keyOf(pair)] === 'delete'" class="mem-note-box">
          Which one goes?
          <button v-for="side in ['a', 'b']" :key="side" class="btn small danger" type="button" :disabled="!!busy || model.isImported(pair[side])" :title="model.isImported(pair[side]) ? 'Imported from ' + pair[side].source + ': edit or delete that file instead' : ''" @click="forget(pair, side)">Forget {{ pair[side].slug }}</button>
        </div>
      </section>
      <details class="mem-unchecked">
        <summary>{{ data.unchecked_total }} imported {{ data.unchecked_total === 1 ? 'memory' : 'memories' }} to check (usually unrelated topics)</summary>
        <p class="muted">An imported memory waits until someone checked it against the memories nearest to it. Check them one by one.</p>
        <button v-if="!checking && current" class="btn" type="button" @click="checking = true">Check one by one</button>
        <div v-if="checking && current" class="mem-check">
          <h4>{{ current.slug }} <span class="muted">({{ model.sourceText(current) }})</span></h4>
          <p>{{ current.summary }}</p>
          <p class="muted">Its nearest memories — how does each relate?</p>
          <div v-for="near in current.nearest" :key="near.slug + near.scope" class="mem-near">
            <div class="mem-near-text"><strong>{{ near.slug }}</strong><span class="muted"> · {{ near.scope }} · {{ model.sourceText(near) }}</span><div>{{ model.unmarked(near.summary) }}</div></div>
            <div class="mem-near-actions" v-if="!checked[current.slug + '/' + near.slug]">
              <button v-for="c in choices(current, near)" :key="c.id" class="btn small" type="button" :title="c.title" :disabled="!!busy" @click="decide(current, near, c)">{{ c.text }}</button>
            </div>
            <div v-else class="mem-ok">{{ checked[current.slug + '/' + near.slug] }}</div>
          </div>
          <p><button class="btn primary" type="button" :disabled="!!busy" @click="done(current)">Checked: next one</button></p>
        </div>
        <p v-if="!current" class="mem-ok">Nothing to check.</p>
      </details>
      <h3 class="mem-rules-heading">Memory vs instruction files</h3>
      <p class="muted">Memories your agent found to contradict a rule of the instruction files its harness loads (CLAUDE.md, AGENTS.md, rules files). graphersal-memory only reads these files.</p>
      <p v-if="!rulePairs.length" class="mem-ok">No memory contradicts your instruction files.</p>
      <details v-if="data.rules_unchecked_total" class="mem-rules-waiting">
        <summary class="muted">{{ model.waitingText(data.rules_unchecked_total) }}</summary>
        <p class="muted">A memory near a rule of an instruction file, not yet checked by your agent (conflicts() in a session). Most are merely on a similar topic; only a contradiction it confirms shows up above.</p>
        <ul class="mem-rules-waiting-list">
          <li v-for="pair in waitingPairs" :key="model.rulePairKey(pair)"><strong>{{ pair.memory.slug }}</strong> <span class="muted">vs {{ model.baseName(pair.rule.file) }} line {{ pair.rule.line }}:</span> {{ pair.rule.text }}</li>
        </ul>
      </details>
      <section v-for="pair in rulePairs" :key="model.rulePairKey(pair)" class="mem-conflict mem-rule-pair" :class="{ 'mem-rule-open': pair.status === 'conflict' }">
        <p class="muted"><strong class="mem-error">They contradict</strong> · since {{ pair.since || 'unknown' }}<span v-if="state.selection === model.ALL"> · {{ model.projectName(pair.project, state.projects) }}</span></p>
        <div class="mem-sides">
          <div class="mem-side">
            <h4>Memory {{ pair.memory.slug }} <span class="muted">({{ model.sourceText(pair.memory) }})</span></h4>
            <p class="muted">{{ pair.memory.kind }} · {{ pair.memory.scope }} · {{ String(pair.memory.updated_at || '').slice(0, 10) }}</p>
            <p>{{ model.unmarked(pair.memory.summary) }}</p>
            <button class="btn small" type="button" @click="edit(pair.memory, pair.project)">Open in Memory</button>
          </div>
          <div class="mem-side">
            <h4>{{ model.baseName(pair.rule.file) }} line {{ pair.rule.line }}</h4>
            <p class="muted mem-path" :title="pair.rule.file">{{ model.fileLine(pair.rule) }} · {{ pair.rule.scope === 'global' ? 'every project' : 'this project' }}<span v-if="model.loadedByText(pair.rule)"> · {{ model.loadedByText(pair.rule) }}</span></p>
            <p>{{ pair.rule.text }}</p>
          </div>
        </div>
        <div class="mem-conflict-actions">
          <button class="btn" type="button" :disabled="!!busy" @click="settleRule(pair, model.ruleActions(pair).compatible, 'Recorded: both hold.')">Both hold</button>
          <button class="btn" type="button" :disabled="!!busy || !model.ruleActions(pair).canForget" :title="model.ruleActions(pair).forgetTitle" @click="forgetMemory(pair)">Memory is outdated</button>
          <button class="btn" type="button" :disabled="!!busy" @click="fileOutdated(pair)">{{ model.ruleActions(pair).fileText }}</button>
          <button class="btn" type="button" @click="later.add(model.rulePairKey(pair))">Later</button>
        </div>
        <div v-if="fileNotes[model.rulePairKey(pair)] || pair.status === 'conflict'" class="mem-note-box">{{ model.ruleActions(pair).fileNote }}</div>
      </section>
    </template>
  </div>`,
};
