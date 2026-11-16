// Run with: node --test server/tests/admin_ui.test.mjs
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const script = readFileSync(new URL('../src/admin.html', import.meta.url), 'utf8')
  .match(/<script>([\s\S]*?)<\/script>/)[1];
const flush = () => new Promise(setImmediate);

function page() {
  const elements = new Map();
  function element() {
    return {
      value: '', dataset: {}, hidden: true, children: [], changes: [], _disabled: false,
      get disabled() { return this._disabled; },
      set disabled(value) {
        if (value !== this._disabled) this.changes.push(value);
        this._disabled = value;
      },
      append(child) { this.children.push(child); },
      replaceChildren(...children) { this.children = children; },
      addEventListener(name, callback) { this[name] = callback; },
    };
  }
  for (const id of ['reload', 'status', 'error', 'systems', 'totals', 'issue-system',
    'previous-issues', 'next-issues', 'issue-rows', 'issue-count']) {
    elements.set(id, element());
  }
  elements.get('reload').disabled = true; // Initial HTML state.
  let interval;
  let phase = 'completed';
  let blockNextCheck;
  const requests = [];
  const context = {
    document: {getElementById: id => elements.get(id), createElement: element},
    AbortSignal, URLSearchParams,
    setInterval(callback) { interval = callback; },
    async fetch(url, {method}) {
      requests.push({url, method});
      if (url.endsWith('/reload') && method === 'GET' && blockNextCheck) {
        const blocked = blockNextCheck;
        blockNextCheck = undefined;
        await blocked;
      }
      if (method === 'POST') phase = 'running';
      const body = url.endsWith('/summary')
        ? [{id: 'dreamcast', name: 'Dreamcast', games: 12, installable: 11}]
        : url.includes('/issues?')
          ? {items: [], total: 0, offset: 0, limit: 50}
          : {status: phase, games: 12, systems: 1, diagnostics: 0};
      return {ok: true, json: async () => body};
    },
  };
  vm.runInNewContext(script, context);
  return {
    elements, requests, context,
    poll: () => interval(),
    setPhase(value) { phase = value; },
    blockCheck() {
      let release;
      blockNextCheck = new Promise(resolve => { release = resolve; });
      return release;
    },
  };
}

test('periodic checks do not toggle the reload button or filter controls', async () => {
  const ui = page();
  await flush();
  const button = ui.elements.get('reload');
  assert.equal(button.disabled, false);
  button.changes.length = 0;
  for (const id of ['issue-system', 'previous-issues', 'next-issues']) {
    ui.elements.get(id).changes.length = 0;
  }
  const release = ui.blockCheck();
  const pending = ui.poll();
  await flush();
  assert.equal(button.disabled, false, 'An idle server stays actionable during GETs');
  assert.deepEqual(button.changes, []);
  release();
  await pending;
  assert.deepEqual(button.changes, [], 'No enabled/disabled flash at the polling interval');
  for (const id of ['issue-system', 'previous-issues', 'next-issues']) {
    assert.deepEqual(ui.elements.get(id).changes, [], `${id} must not flash either`);
  }
});

test('a reload click during a background check is queued exactly once', async () => {
  const ui = page();
  await flush();
  const release = ui.blockCheck();
  const pending = ui.poll();
  await flush();
  await ui.elements.get('reload').click();
  assert.equal(ui.elements.get('reload').disabled, true);
  release();
  await pending;
  await flush();
  assert.equal(ui.requests.filter(request => request.method === 'POST').length, 1);
  assert.equal(ui.elements.get('status').dataset.running, 'true');
  const button = ui.elements.get('reload');
  button.changes.length = 0;
  await ui.poll();
  assert.equal(button.disabled, true);
  assert.deepEqual(button.changes, []);
  ui.setPhase('completed');
  await ui.poll();
  assert.equal(button.disabled, false);
  assert.deepEqual(button.changes, [false]);
});

test('failed checks disable reload safely and a successful retry restores it', async () => {
  const ui = page();
  await flush();
  const fetch = ui.context.fetch;
  ui.context.fetch = async () => { throw new Error('offline'); };
  await ui.poll();
  assert.equal(ui.elements.get('error').hidden, false);
  assert.equal(ui.elements.get('reload').disabled, true);
  ui.context.fetch = fetch;
  await ui.poll();
  assert.equal(ui.elements.get('error').hidden, true);
  assert.equal(ui.elements.get('reload').disabled, false);
});
