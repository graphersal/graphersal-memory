// node --test crates/graphersal-memory/ui/tests/topbar.test.mjs
// The project choice holds for every tab, so it lives in the page's top bar (the profile's
// `header` module, ui/topbar.js), not in each tab's header (ui/shared.js `Header`).

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { problemsText } from '../model.js';

const ui = new URL('../', import.meta.url);
const read = (name) => readFileSync(new URL(name, ui), 'utf8');

test('the profile names the top-bar module, and it is a file of the profile that is built in', () => {
  const profile = JSON.parse(read('profile.json'));
  assert.deepEqual(profile.header, { module: 'topbar.js' });
  assert.ok(existsSync(new URL(profile.header.module, ui)));
  assert.ok(read('../src/ui/ui_assets.rs').includes('("topbar.js", include_bytes!("../../ui/topbar.js"))'), 'served under /memory-ui/');
});

test('the Project select is in the top bar only; it sets the shared selection', () => {
  const topbar = read('topbar.js');
  assert.ok(topbar.includes('aria-label="Project"'));
  assert.ok(/import \{[^}]*\bselect\b[^}]*\} from '\.\/shared\.js'/.test(topbar), 'the shared select()');
  for (const option of ['model.ALL', 'model.GLOBAL', 'state.projects']) assert.ok(topbar.includes(option), option);
  const shared = read('shared.js');
  const header = shared.slice(shared.indexOf('export const Header'), shared.indexOf('export const NearestCheck'));
  assert.ok(!header.includes('<select'), 'no project select in a tab header');
  for (const tab of ['overview.js', 'memory.js', 'graph.js', 'always.js', 'conflicts.js', 'sessions.js']) {
    assert.ok(!read(tab).includes('aria-label="Project"'), `${tab} has no project select of its own`);
  }
});

test('setup problems in a few words', () => {
  assert.equal(problemsText(1), '1 setup problem');
  assert.equal(problemsText(3), '3 setup problems');
});
