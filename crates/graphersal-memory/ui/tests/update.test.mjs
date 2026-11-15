// node --test crates/graphersal-memory/ui/tests/update.test.mjs
// A newer release: a notice in the top bar that opens the Overview, where the Overview shows how
// to upgrade (the `upgrade` command and the manual commands, each with a Copy button).

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { updateText } from '../model.js';

const ui = new URL('../', import.meta.url);
const read = (name) => readFileSync(new URL(name, ui), 'utf8');

test('the notice appears only for a newer release with the check on', () => {
  const newer = { enabled: true, newer: true, current: '0.2.0', latest: '0.3.0' };
  assert.equal(updateText(newer), 'Version 0.3.0 available');
  assert.equal(updateText({ ...newer, newer: false }), '');
  assert.equal(updateText({ ...newer, enabled: false }), '');
  assert.equal(updateText({ ...newer, latest: null }), '');
  assert.equal(updateText(null), '');
});

test('the check is asked for once per page and shown in the top bar and the Overview', () => {
  assert.ok(read('api-ui.js').includes("call('/api/ui/update')"));
  const shared = read('shared.js');
  assert.ok(shared.includes('getUpdate().then('), 'loaded with the projects, once');
  const topbar = read('topbar.js');
  assert.ok(topbar.includes('model.updateText(state.update)') && topbar.includes('@click="openOverview"'));
  const overview = read('overview.js');
  for (const part of ['state.update.upgrade', 'state.update.manual', 'state.update.release_notes', 'state.update.switch_off', 'navigator.clipboard.writeText']) {
    assert.ok(overview.includes(part), part);
  }
});
