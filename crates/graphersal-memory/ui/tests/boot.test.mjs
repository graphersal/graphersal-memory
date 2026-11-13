// node --test crates/graphersal-memory/ui/tests/boot.test.mjs
// ui/boot.js hands the token to the playground's engine through its host hook
// (`window.GRAPHERSAL_ENGINE = {headers}`) instead of replacing `window.fetch`; ui/api-ui.js
// sends the same headers to the `/api/ui/*` endpoints.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { hostHeaders } from '../api-ui.js';

const BOOT = readFileSync(new URL('../boot.js', import.meta.url), 'utf8');
const TOKEN = 'ab'.repeat(32);

/** Runs boot.js in a fake page; returns the window and the requests it sent. */
function runBoot(hash, stored = null) {
  const requests = [];
  const storage = new Map(stored ? [['graphersal-memory.ui.token', stored]] : []);
  const replaced = [];
  const fetch = (url, init) => {
    requests.push({ url, init });
    return new Promise(() => {});
  };
  const window = {
    location: { hash, pathname: '/', href: `http://127.0.0.1:1/${hash}`, origin: 'http://127.0.0.1:1' },
    history: { replaceState: (_s, _t, url) => replaced.push(url) },
    sessionStorage: { getItem: (k) => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v) },
    setInterval: () => 0,
    fetch,
  };
  const document = {
    body: null,
    addEventListener: () => {},
    getElementById: () => null,
    visibilityState: 'visible',
  };
  vm.runInNewContext(BOOT, { window, document, URL, Headers: globalThis.Headers, Object });
  return { window, requests, replaced, storage, fetch };
}

test('the token from the address becomes the engine hook, fetch is left alone', () => {
  const page = runBoot(`#token=${TOKEN}&tab=graph`);
  assert.equal(JSON.stringify(page.window.GRAPHERSAL_ENGINE), JSON.stringify({ headers: { Authorization: `Bearer ${TOKEN}` } }));
  assert.equal(page.window.fetch, page.fetch, 'window.fetch is not replaced');
  assert.equal(page.storage.get('graphersal-memory.ui.token'), TOKEN);
  assert.equal(JSON.stringify(page.replaced), JSON.stringify(['/#/play?tab=graph']), 'the token leaves the address');
  // The heartbeat carries the header itself; the token is never in its URL.
  assert.equal(page.requests.length, 1);
  assert.equal(page.requests[0].url, '/api/ui/heartbeat');
  assert.equal(page.requests[0].init.headers.Authorization, `Bearer ${TOKEN}`);
});

test('a reload takes the token from sessionStorage; without one there is no hook', () => {
  const again = runBoot('#/play?tab=memory', TOKEN);
  assert.equal(again.window.GRAPHERSAL_ENGINE.headers.Authorization, `Bearer ${TOKEN}`);
  const none = runBoot('');
  assert.equal(none.window.GRAPHERSAL_ENGINE, undefined);
  assert.equal(none.requests.length, 0);
});

test('api-ui sends the headers boot.js set', () => {
  assert.deepEqual(hostHeaders({}), {});
  assert.deepEqual(hostHeaders({ GRAPHERSAL_ENGINE: { headers: { Authorization: 'Bearer x' } } }), { Authorization: 'Bearer x' });
  const copy = hostHeaders({ GRAPHERSAL_ENGINE: { headers: { A: '1' } } });
  copy.B = '2';
  assert.equal(Object.keys(copy).length, 2, 'a copy, the page config is not changed');
});
