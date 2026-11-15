// The memory profile's top-bar module (profile.json `header`): the choices and states that hold
// for every tab, in the page's top bar instead of inside each tab. The Project select (All
// projects, Global, each project) sets the shared selection (shared.js `state.selection`, kept in
// localStorage), which every tab watches; the daemon's state; the setup problems as a warning
// that opens the Overview (which lists them). Texts other than the select hide on narrow screens
// (the playground's `hide-narrow`), so the bar never overflows.

import { computed } from 'vue';
import * as model from './model.js';
import { state, init, select } from './shared.js';

export default {
  props: { api: { type: Object, required: true } },
  setup(props) {
    init();
    const problems = computed(() => model.healthProblems(state.status));
    return {
      state,
      model,
      problems,
      choose: select,
      openOverview: () => props.api.navigate('overview'),
    };
  },
  template: `
  <div class="mem-topbar">
    <span v-if="state.statusError" class="mem-error mem-topbar-error" role="alert" :title="state.statusError">✗<span class="hide-narrow"> {{ state.statusError }}</span></span>
    <button v-if="problems.length" type="button" class="mem-problems mem-topbar-problems" :title="problems.join('\\n') + '\\n\\nClick: the Overview lists them.'"
            @click="openOverview">⚠ {{ model.problemsText(problems.length) }}</button>
    <button v-if="model.updateText(state.update)" type="button" class="mem-topbar-update" :title="'You have ' + state.update.current + '. Click: how to upgrade.'"
            @click="openOverview">⬆ <span class="hide-narrow">{{ model.updateText(state.update) }}</span><span class="show-narrow">{{ state.update.latest }}</span></button>
    <span class="mem-daemon hide-narrow" :class="{bad: !state.status || !state.status.daemon}">{{ model.daemonText(state.status) }}</span>
    <label class="mem-switch">
      <span class="muted hide-narrow">Project</span>
      <select :value="state.selection" @change="choose($event.target.value)" aria-label="Project">
        <option :value="model.ALL">All projects</option>
        <option :value="model.GLOBAL">Global (every project)</option>
        <option v-for="p in state.projects" :key="p.id" :value="p.id" :title="p.paths && p.paths[0]">{{ p.name }}</option>
      </select>
    </label>
  </div>`,
};
