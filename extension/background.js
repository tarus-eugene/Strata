export const HOST = 'com.strata.download_manager';
const pendingPrefix = 'handoff:';
const active = new Set();

export function remoteLink(input) {
  try { const url = new URL(input); return ['http:', 'https:', 'ftp:', 'sftp:', 'webdav:', 'webdavs:', 'magnet:'].includes(url.protocol); }
  catch { return false; }
}

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
    filename: (item.filename || '').split(/[\\/]/).pop() || null, referrer,
    ...(Number.isSafeInteger(item.totalBytes) && item.totalBytes >= 0 ? { size: item.totalBytes } : {}) };
}

function requestDownload(api, request) {
  // A native port keeps the service worker alive while the desktop dialog is open.
  return new Promise((resolve, reject) => {
    const port = api.runtime.connectNative(HOST);
    port.onMessage.addListener(reply => {
      resolve(reply);
      port.disconnect();
    });
    port.onDisconnect.addListener(() => {
      reject(new Error(api.runtime.lastError?.message || '101 Downloader disconnected.'));
    });
    try { port.postMessage(request); }
    catch (error) { port.disconnect(); reject(error); }
  });
}

export function usefulFilename(filename) {
  const name = (filename || '').split(/[\\/]/).pop();
  return !!name && !/^(?:\{?[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\}?|[0-9a-f]{32,64})(?:\.[^.]+)?$/i.test(name);
}

export async function setDownloadUi(api, enabled) {
  try { await api.downloads.setUiOptions({ enabled }); return true; }
  catch { return false; } // Some Chromium forks do not implement downloads.ui.
}

export async function syncDownloadUi(api) {
  const { captureEnabled = true } = await api.storage.local.get('captureEnabled');
  return setDownloadUi(api, !captureEnabled);
}

async function record(api, message, error = false) {
  await api.storage.local.set({ lastResult: { message, error, time: Date.now() } });
  await api.action.setBadgeText({ text: error ? '!' : '' });
  if (error) await api.action.setBadgeBackgroundColor({ color: '#9f4049' });
}

export async function captureDownload(item, api, onPaused) {
  if (!eligible(item)) {
    if (item.state === 'in_progress') await setDownloadUi(api, true);
    return false;
  }
  if (item.byExtensionId === api.runtime.id || active.has(item.id)) return false;
  const key = `${pendingPrefix}${item.id}`;
  active.add(item.id);
  let paused = false;
  let accepted = false;
  let cancelled = false;
  let browserCleared = false;
  try {
    const { captureEnabled = true } = await api.storage.local.get('captureEnabled');
    if (!captureEnabled) return false;
    await setDownloadUi(api, false);
    // Persist recovery state before pausing; a suspended worker must not strand Chrome's transfer.
    await api.storage.local.set({ [key]: { id: item.id, startTime: item.startTime, phase: 'pending' } });
    await api.downloads.pause(item.id);
    paused = true;
    const [current] = await api.downloads.search({ id: item.id });
    if (!current || !current.paused || !eligible({ ...item, ...current, paused: false })) {
      if (current?.state === 'in_progress' && current.paused) await api.downloads.resume(item.id);
      await setDownloadUi(api, true);
      return false;
    }
    onPaused?.();
    const download = { ...item, ...current, filename: usefulFilename(current.filename) ? current.filename : (item.filename || current.filename) };
    const reply = await requestDownload(api, nativeRequest(download));
    if (reply?.cancelled === true) {
      cancelled = true;
      await api.storage.local.set({ [key]: { id: item.id, startTime: item.startTime, phase: 'cancelled' } });
      await api.downloads.cancel(item.id);
      browserCleared = true;
      await record(api, 'Download cancelled in 101 Downloader.');
      return false;
    }
    if (reply?.ok !== true || !Number.isSafeInteger(reply.id) || reply.id <= 0) {
      throw new Error(reply?.error || '101 Downloader did not accept the download.');
    }
    accepted = true;
    await api.storage.local.set({ [key]: { id: item.id, startTime: item.startTime, phase: 'accepted', appId: reply.id } });
    try {
      await api.downloads.cancel(item.id);
      browserCleared = true;
      await record(api, `Sent ${nativeRequest({ ...item, ...current }).filename || 'download'} to 101 Downloader.`);
    } catch {
      await record(api, 'Sent to 101 Downloader. Your browser could not clear its paused copy; you can cancel it from its downloads page.', true);
    }
    return true;
  } catch (error) {
    await setDownloadUi(api, true);
    if (cancelled) {
      await record(api, 'Open your browser downloads page to cancel its paused copy.', true);
      return false;
    }
    let message = 'Kept in your browser. ' + error.message;
    if (paused && !accepted) {
      try { await api.downloads.resume(item.id); }
      catch { message = 'Open your browser downloads page to resume this file. ' + error.message; }
    }
    await record(api, message, true);
    return false;
  } finally {
    try {
      if (browserCleared || (!accepted && !cancelled)) await api.storage.local.remove(key);
    } finally { active.delete(item.id); }
  }
}

export async function captureChanged(delta, api) {
  if (!['safe', 'deepScannedSafe'].includes(delta.danger?.current)) return false;
  const [item] = await api.downloads.search({ id: delta.id });
  return item ? captureDownload(item, api) : false;
}

export async function recoverPending(api) {
  const stored = await api.storage.local.get(null);
  for (const [key, pending] of Object.entries(stored)) {
    if (!key.startsWith(pendingPrefix) || !Number.isSafeInteger(pending?.id)) continue;
    try {
      const [item] = await api.downloads.search({ id: pending.id });
      if (typeof pending.startTime === 'string' && item?.startTime === pending.startTime && item.state === 'in_progress' && item.paused) {
        if (pending.phase === 'accepted' || pending.phase === 'cancelled') await api.downloads.cancel(pending.id);
        else { await api.downloads.resume(pending.id); await setDownloadUi(api, true); }
      }
      await api.storage.local.remove(key);
    } catch {
      await setDownloadUi(api, true);
      // Retain recovery state so the next worker activation can try again.
    }
  }
}

if (globalThis.chrome?.runtime?.id && typeof document === 'undefined') {
  const ready = syncDownloadUi(chrome).then(() => recoverPending(chrome));
  chrome.runtime.onStartup.addListener(() => { syncDownloadUi(chrome).catch(error => record(chrome, error.message, true)); });
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area === 'local' && changes.captureEnabled) syncDownloadUi(chrome).catch(error => record(chrome, error.message, true));
  });
  chrome.runtime.onInstalled.addListener(async () => {
    await syncDownloadUi(chrome);
    await chrome.contextMenus.removeAll();
    chrome.contextMenus.create({ id: 'send-to-strata', title: 'Download with 101 Downloader',
      contexts: ['link'] });
  });
  chrome.downloads.onCreated.addListener(item => {
    // Suppress UI immediately, but wait for Chrome's filename before handing off.
    ready.then(() => eligible(item) ? syncDownloadUi(chrome) : setDownloadUi(chrome, true)).catch(error => record(chrome, error.message, true));
  });
  chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
    let suggested = false;
    const release = () => {
      if (!suggested) { suggested = true; suggest(); }
    };
    ready.then(() => captureDownload(item, chrome, release)).catch(error => record(chrome, error.message, true)).finally(release);
    return true;
  });
  chrome.downloads.onChanged.addListener(delta => {
    ready.then(() => captureChanged(delta, chrome)).catch(error => record(chrome, error.message, true));
  });
  chrome.contextMenus.onClicked.addListener(async info => {
    if (info.menuItemId !== 'send-to-strata') return;
    try {
      if (!remoteLink(info.linkUrl)) throw new Error('Choose an HTTP, FTP, SFTP, WebDAV, or magnet download link.');
      const reply = await requestDownload(chrome, {
        action: 'download', url: info.linkUrl,
        referrer: /^https?:\/\//i.test(info.pageUrl || '') ? info.pageUrl : null,
      });
      if (reply?.cancelled === true) return;
      if (!reply?.ok) throw new Error(reply?.error || '101 Downloader did not accept this link.');
      await record(chrome, 'Link sent to 101 Downloader.');
    } catch (error) { await record(chrome, error.message, true); }
  });
}
