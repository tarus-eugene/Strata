import test from 'node:test';
import assert from 'node:assert/strict';
import { captureDownload, captureChanged, eligible, recoverPending, remoteLink, usefulFilename, syncDownloadUi } from './background.js';

test('context menu accepts remote protocols while refusing local files and executable schemes', () => {
  for (const url of ['https://host/a', 'ftp://host/a', 'sftp://user@host/a', 'webdav://host/a', 'magnet:?xt=urn:btih:abc']) assert.equal(remoteLink(url), true);
  for (const url of ['file:///C:/private.torrent', 'javascript:alert(1)', 'data:text/plain,secret', 'not a URL']) assert.equal(remoteLink(url), false);
});

const item = { id: 7, url: 'https://example.com/file.bin', filename: 'C:\\Downloads\\file.bin',
  state: 'in_progress', danger: 'safe', paused: false, referrer: 'https://example.com/files', startTime: '2026-10-30T10:00:00.000Z' };

function mock(reply = { ok: true, id: 42 }) {
  const calls = []; const values = {}; let current = { ...item };
  const api = {
    calls, values, ui: [],
    runtime: { id: 'strata', async sendNativeMessage(host, request) {
      calls.push(['native', request]); if (reply instanceof Error) throw reply; return reply;
    } },
    storage: { local: {
      async get(keys) { if (keys === null) return { ...values }; return Object.fromEntries((Array.isArray(keys) ? keys : [keys]).map(k => [k, values[k]])); },
      async set(data) { Object.assign(values, data); }, async remove(key) { delete values[key]; },
    } },
    downloads: {
      async setUiOptions(options) { api.ui.push(options.enabled); },
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
  assert.deepEqual(api.ui, [false]);
});

test('resumes Chrome if the app rejects the request or the bridge is absent', async () => {
  for (const reply of [{ ok: false, error: 'No app' }, new Error('Native host not found'), { ok: true }]) {
    const api = mock(reply); assert.equal(await captureDownload(item, api), false);
    assert.deepEqual(api.calls.map(c => c[0]), ['pause', 'native', 'resume']);
    assert.equal(api.values.lastResult.error, true);
    assert.deepEqual(api.ui, [false, true]);
  }
});

test('suppresses the browser download UI with capture on and restores it with capture off', async () => {
  const api = mock();
  assert.equal(await syncDownloadUi(api), true);
  api.values.captureEnabled = false;
  assert.equal(await syncDownloadUi(api), true);
  assert.deepEqual(api.ui, [false, true]);
});

test('an unavailable download UI API never prevents a successful handoff', async () => {
  const api = mock(); api.downloads.setUiOptions = async () => { throw Error('Unsupported'); };
  assert.equal(await captureDownload(item, api), true);
});

test('leaves browser UI available for downloads Strata cannot capture', async () => {
  const api = mock();
  await captureDownload({ ...item, url: 'blob:https://example.com/id' }, api);
  assert.deepEqual(api.ui, [true]);
});

test('recognizes temporary names and preserves the browser filename-determination result', async () => {
  const temporary = '0976f308-1892-4a34-aeca-7b155f43121e';
  assert.equal(usefulFilename(temporary), false);
  assert.equal(usefulFilename('a'.repeat(32) + '.zip'), false);
  assert.equal(usefulFilename('Annual report 2026.pdf'), true);
  const api = mock(); api.setCurrent({ ...item, filename: temporary });
  assert.equal(await captureDownload({ ...item, filename: 'Annual report 2026.pdf' }, api), true);
  assert.equal(api.calls.find(c => c[0] === 'native')[1].filename, 'Annual report 2026.pdf');
});

test('keeps Chrome untouched when capture is disabled', async () => {
  const api = mock(); api.values.captureEnabled = false;
  assert.equal(await captureDownload(item, api), false); assert.deepEqual(api.calls, []);
});

test('concurrent automatic events hand a browser download off only once', async () => {
  const api = mock();
  const results = await Promise.all([captureDownload(item, api), captureDownload(item, api)]);
  assert.equal(results.filter(Boolean).length, 1);
  assert.equal(api.calls.filter(c => c[0] === 'native').length, 1);
});

test('captures automatically once browser scanning reports the download safe', async () => {
  const api = mock();
  assert.equal(await captureDownload({ ...item, danger: 'asyncScanning' }, api), false);
  assert.equal(await captureChanged({ id: item.id, danger: { current: 'safe' } }, api), true);
  assert.deepEqual(api.calls.map(c => c[0]), ['pause', 'native', 'cancel']);
});

test('does not hand off a download that becomes unsafe while it is being paused', async () => {
  const api = mock(); api.setCurrent({ ...item, danger: 'url' });
  assert.equal(await captureDownload(item, api), false);
  assert.deepEqual(api.calls.map(c => c[0]), ['pause', 'resume']);
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

test('worker waits for filename determination, suggests once, and follows the capture toggle', async () => {
  const api = mock(); const listeners = {};
  const event = name => ({ addListener(callback) { listeners[name] = callback; } });
  api.runtime.onInstalled = event('install'); api.runtime.onStartup = event('startup');
  api.storage.onChanged = event('storage');
  api.downloads.onCreated = event('created'); api.downloads.onChanged = event('changed');
  api.downloads.onDeterminingFilename = event('filename');
  api.contextMenus = { onClicked: event('context'), async removeAll() {}, create() {} };
  const previous = globalThis.chrome; globalThis.chrome = api;
  try {
    await import('./background.js?worker-wiring');
    await new Promise(resolve => setImmediate(resolve));
    listeners.created({ ...item, filename: '' });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(api.calls.length, 0);
    api.setCurrent({ ...item, filename: 'Final report.pdf' });
    let suggests = 0;
    await new Promise(resolve => {
      assert.equal(listeners.filename({ ...item, filename: 'Final report.pdf' }, () => { suggests++; resolve(); }), true);
    });
    assert.equal(suggests, 1);
    assert.equal(api.calls.find(c => c[0] === 'native')[1].filename, 'Final report.pdf');
    assert.deepEqual(api.calls.map(c => c[0]), ['pause', 'native', 'cancel']);
    api.values.captureEnabled = false;
    listeners.storage({ captureEnabled: { newValue: false } }, 'local');
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(api.ui.at(-1), true);
  } finally { if (previous === undefined) delete globalThis.chrome; else globalThis.chrome = previous; }
});