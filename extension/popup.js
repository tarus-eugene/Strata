import { HOST } from './background.js';

const $ = id => document.getElementById(id);
function result(message, error = false) { $('result').textContent = message; $('result').classList.toggle('error', error); }

async function check() {
  $('refresh').disabled = true;
  try {
    const reply = await chrome.runtime.sendNativeMessage(HOST, { action: 'ping' });
    if (!reply?.ok) throw new Error(reply?.error || 'Open Strata to connect.');
    $('status').textContent = `Connected to Strata ${reply.version}`;
    $('status-dot').classList.add('connected'); $('setup').hidden = true;
  } catch {
    $('status').textContent = 'App not connected'; $('status-dot').classList.remove('connected'); $('setup').hidden = false;
  } finally { $('refresh').disabled = false; }
}

const stored = await chrome.storage.local.get(['captureEnabled', 'lastResult']);
$('capture').checked = stored.captureEnabled !== false;
if (stored.lastResult) result(stored.lastResult.message, stored.lastResult.error);
$('capture').addEventListener('change', async () => {
  await chrome.storage.local.set({ captureEnabled: $('capture').checked });
  result($('capture').checked ? 'New file downloads will be sent to Strata.' : 'New downloads will stay in your browser.');
});
$('refresh').addEventListener('click', check);
$('send-form').addEventListener('submit', async event => {
  event.preventDefault(); $('send').disabled = true;
  try {
    const reply = await chrome.runtime.sendNativeMessage(HOST, { action: 'download', url: $('url').value.trim() });
    if (!reply?.ok) throw new Error(reply?.error || 'Strata did not accept this link.');
    result('Download added to Strata.'); $('url').value = ''; await check();
  } catch (error) { result(error.message, true); }
  finally { $('send').disabled = false; }
});
$('open-app').addEventListener('click', async () => {
  try {
    const reply = await chrome.runtime.sendNativeMessage(HOST, { action: 'open' });
    if (!reply?.ok) throw new Error(reply?.error || 'Could not open Strata.');
    await check();
  } catch (error) { result(error.message, true); }
});
$('help').addEventListener('click', () => chrome.tabs.create({ url: chrome.runtime.getURL('setup.html') }));
await check();
