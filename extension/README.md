# 101 Downloader browser companion

1. Install 101 Downloader with its browser connection task enabled, or open 101 Downloader **Settings → Browser companion** and click **Connect browsers**. Source builds can run `install-browser.ps1` from the project root.
2. Open your Chromium browser's extensions page and enable **Developer mode**: Chrome / Chromium `chrome://extensions`, Edge `edge://extensions`, Brave `brave://extensions`, Vivaldi `vivaldi://extensions`, Opera / Opera GX `opera://extensions`.
3. Click **Load unpacked** and select this folder.
4. Pin the 101 Downloader extension and check that the popup says **Connected to 101 Downloader**. Repeat in each browser profile you use.

Capture is enabled by default. Start an eligible HTTP/HTTPS file download normally: it is sent to 101 Downloader automatically, and 101 Downloader opens or restores from the tray. No right-click command is required. Downloads awaiting browser scanning are handed off when scanning reports them safe. Turn **Capture downloads** off to keep downloads in your browser. Pasting a link or choosing **Download with 101 Downloader** remains an optional shortcut.

The extension pauses Chrome's copy while it asks 101 Downloader to accept the file. Chrome's copy is canceled only after the app confirms acceptance. If the bridge or app is unavailable, the browser download resumes.

Private downloads, browser-generated files, and files flagged by Chrome remain in Chrome. Cookies and passwords are not exported. Login-only or POST-generated files may need capture turned off. Errors after an accepted handoff appear in 101 Downloader.

The desktop app and `101Downloader-Bridge.exe` (or `strata-bridge.exe` in source builds) must remain beside each other. If you move them, reconnect the bridge. The extension ID stays fixed through the public key in `manifest.json`.

The same Manifest V3 extension uses standard Chromium APIs across compatible Windows browsers. Chrome, Chromium, and Edge host registration locations are configured. Browser policies or fork-specific restrictions can disable native messaging or unpacked extensions. Every Chromium fork has not been tested individually; the native app is Windows-only.

To disconnect, turn capture off and remove the extension in Chrome. `install-browser.ps1 -Uninstall` unregisters the app connection without deleting downloads or history.

While capture is enabled, the companion suppresses Chromium download UI using the downloads.ui permission. Reload the updated extension and approve that permission if your browser asks. Capture off or browser fallback restores the UI. This control is profile-wide and may be unavailable in some Chromium forks. Filename determination is awaited before handoff so temporary GUID names can be replaced by the browser name; 101 Downloader also reads server filename metadata.
