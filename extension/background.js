export const HOST = 'com.strata.download_manager';
const pendingPrefix = 'handoff:';
const active = new Set();

export function eligible(item) {
  try {
    const url = new URL(item.finalUrl || item.url);
    return ['http:', 'https:'].includes(url.protocol) && !url.username && !url.password &&
      item.state === 'in_progress' && !item.paused && !item.incognito &&
      (!item.danger || item.danger === 'safe' || item.danger === 'deepScannedSafe');
  } catch { return false; }
}

function nativeRequest(item) {
  const referrer = /^https?:\/\//i.test(item.referrer || '') ? item.referrer : null;
  return { action: 'download', url: item.finalUrl || item.url,
    filename: (item.filename || '').split(/[\\/]/).pop() || null, referrer };
}

async function record(api, message, error = false) {
  await api.storage.local.set({ lastResult: { message, error, time: Date.now() } });
  await api.action.setBadgeText({ text: error ? '!' : '' });
  if (error) await api.action.setBadgeBackgroundColor({ color: '#9f4049' });
}

export async function captureDownload(item, api) {
  if (!eligible(item) || item.byExtensionId === api.runtime.id || active.has(item.id)) return false;
  const { captureEnabled = true } = await api.storage.local.get('captureEnabled');
  if (!captureEnabled) return false;
  const key = `${pendingPrefix}${item.id}`;
  active.add(item.id);
  let paused = false;
  let accepted = false;
  try {
    // Persist recovery state before pausing; a suspended worker must not strand Chrome's transfer.
    await api.storage.local.set({ [key]: { id: item.id, startTime: item.startTime, phase: 'pending' } });
    await api.downloads.pause(item.id);
    paused = true;
    const [current] = await api.downloads.search({ id: item.id });
    if (!current || current.state !== 'in_progress' || !current.paused) return false;
    const reply = await api.runtime.sendNativeMessage(HOST, nativeRequest({ ...item, ...current }));
    if (reply?.ok !== true || !Number.isSafeInteger(reply.id) || reply.id <= 0) {
      throw new Error(reply?.error || 'Strata did not accept the download.');
    }
    accepted = true;
    await api.storage.local.set({ [key]: { id: item.id, startTime: item.startTime, phase: 'accepted', appId: reply.id } });
    try {
      await api.downloads.cancel(item.id);
      await record(api, `Sent ${nativeRequest({ ...item, ...current }).filename || 'download'} to Strata.`);
    } catch {
      await record(api, 'Sent to Strata. Chrome could not clear its paused copy; you can cancel it in Chrome.', true);
    }
    return true;
  } catch (error) {
    let message = 'Kept in Chrome. ' + error.message;
    if (paused && !accepted) {
      try { await api.downloads.resume(item.id); }
      catch { message = 'Open Chrome Downloads to resume this file. ' + error.message; }
    }
    await record(api, message, true);
    return false;
  } finally {
    await api.storage.local.remove(key);
    active.delete(item.id);
  }
}

export async function recoverPending(api) {
  const stored = await api.storage.local.get(null);
  for (const [key, pending] of Object.entries(stored)) {
    if (!key.startsWith(pendingPrefix) || !Number.isSafeInteger(pending?.id)) continue;
    try {
      const [item] = await api.downloads.search({ id: pending.id });
      if (typeof pending.startTime === 'string' && item?.startTime === pending.startTime && item.state === 'in_progress' && item.paused) {
        if (pending.phase === 'accepted') await api.downloads.cancel(pending.id);
        else await api.downloads.resume(pending.id);
      }
      await api.storage.local.remove(key);
    } catch { /* Retain recovery state so the next worker activation can try again. */ }
  }
}

if (globalThis.chrome?.runtime?.id && typeof document === 'undefined') {
  const ready = recoverPending(chrome);
  chrome.runtime.onInstalled.addListener(async () => {
    await chrome.contextMenus.removeAll();
    chrome.contextMenus.create({ id: 'send-to-strata', title: 'Download with Strata',
      contexts: ['link'], targetUrlPatterns: ['http://*/*', 'https://*/*'] });
  });
  chrome.downloads.onCreated.addListener(item => {
    ready.then(() => captureDownload(item, chrome)).catch(error => record(chrome, error.message, true));
  });
  chrome.contextMenus.onClicked.addListener(async info => {
    if (info.menuItemId !== 'send-to-strata') return;
    try {
      const reply = await chrome.runtime.sendNativeMessage(HOST, {
        action: 'download', url: info.linkUrl,
        referrer: /^https?:\/\//i.test(info.pageUrl || '') ? info.pageUrl : null,
      });
      if (!reply?.ok) throw new Error(reply?.error || 'Strata did not accept this link.');
      await record(chrome, 'Link sent to Strata.');
    } catch (error) { await record(chrome, error.message, true); }
  });
}
