import test from 'node:test';
import assert from 'node:assert/strict';
import { captureDownload, eligible, recoverPending } from './background.js';

const item = { id: 7, url: 'https://example.com/file.bin', filename: 'C:\\Downloads\\file.bin',
  state: 'in_progress', danger: 'safe', paused: false, referrer: 'https://example.com/files', startTime: '2026-10-30T10:00:00.000Z' };

function mock(reply = { ok: true, id: 42 }) {
  const calls = []; const values = {}; let current = { ...item };
  const api = {
    calls, values,
    runtime: { id: 'strata', async sendNativeMessage(host, request) {
      calls.push(['native', request]); if (reply instanceof Error) throw reply; return reply;
    } },
    storage: { local: {
      async get(keys) { if (keys === null) return { ...values }; return Object.fromEntries((Array.isArray(keys) ? keys : [keys]).map(k => [k, values[k]])); },
      async set(data) { Object.assign(values, data); }, async remove(key) { delete values[key]; },
    } },
    downloads: {
      async pause(id) { calls.push(['pause', id]); current.paused = true; },
      async search() { return [{ ...current }]; },
      async cancel(id) { calls.push(['cancel', id]); current.state = 'interrupted'; },
      async resume(id) { calls.push(['resume', id]); current.paused = false; },
    },
    action: { async setBadgeText() {}, async setBadgeBackgroundColor() {} },
    setCurrent(value) { current = value; },
  };
  return api;
}

test('cancels Chrome only after a confirmed app handoff, sending a basename rather than a local path', async () => {
  const api = mock(); assert.equal(await captureDownload(item, api), true);
  assert.deepEqual(api.calls.map(c => c[0]), ['pause', 'native', 'cancel']);
  assert.deepEqual(api.calls[1][1], { action: 'download', url: item.url, filename: 'file.bin', referrer: item.referrer });
  assert.equal(api.values['handoff:7'], undefined);
});

test('resumes Chrome if the app rejects the request or the bridge is absent', async () => {
  for (const reply of [{ ok: false, error: 'No app' }, new Error('Native host not found'), { ok: true }]) {
    const api = mock(reply); assert.equal(await captureDownload(item, api), false);
    assert.deepEqual(api.calls.map(c => c[0]), ['pause', 'native', 'resume']);
    assert.equal(api.values.lastResult.error, true);
  }
});

test('keeps Chrome untouched when capture is disabled', async () => {
  const api = mock(); api.values.captureEnabled = false;
  assert.equal(await captureDownload(item, api), false); assert.deepEqual(api.calls, []);
});

test('skips private, unsafe, completed, paused and browser-generated downloads', async () => {
  for (const change of [{ incognito: true }, { danger: 'url' }, { danger: 'asyncScanning' }, { state: 'complete' }, { paused: true }, { url: 'blob:https://example.com/id' }, { url: 'file:///private' }, { url: 'https://user:pass@example.com' }]) {
    assert.equal(eligible({ ...item, ...change }), false);
    const api = mock(); await captureDownload({ ...item, ...change }, api); assert.deepEqual(api.calls, []);
  }
});

test('does not resume Chrome when its cancellation fails after the app has accepted', async () => {
  const api = mock(); api.downloads.cancel = async id => { api.calls.push(['cancel', id]); throw Error('Busy'); };
  assert.equal(await captureDownload(item, api), true);
  assert.equal(api.calls.some(c => c[0] === 'resume'), false);
});

test('recovers paused handoffs after worker restart without resending to the app', async () => {
  for (const phase of ['pending', 'accepted']) {
    const api = mock(); api.values['handoff:7'] = { id: 7, startTime: item.startTime, phase }; api.setCurrent({ ...item, paused: true });
    await recoverPending(api);
    assert.deepEqual(api.calls, [[phase === 'accepted' ? 'cancel' : 'resume', 7]]);
    assert.equal(api.values['handoff:7'], undefined);
  }
});

test('retains recovery state when Chrome is temporarily unavailable', async () => {
  const api = mock(); api.values['handoff:7'] = { id: 7, phase: 'pending' };
  api.downloads.search = async () => { throw Error('Browser starting'); };
  await recoverPending(api); assert.ok(api.values['handoff:7']);
});

test('does not act on a different download that reuses an old handoff ID', async () => {
  const api = mock();
  api.values['handoff:7'] = { id: 7, startTime: '2020-01-01T00:00:00.000Z', phase: 'accepted' };
  api.setCurrent({ ...item, paused: true });
  await recoverPending(api);
  assert.deepEqual(api.calls, []); assert.equal(api.values['handoff:7'], undefined);
});
