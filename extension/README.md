# Strata Chrome companion

1. Open Strata. In **Settings → Chrome companion**, click **Connect Chrome** (or run `install-browser.ps1` from the project root).
2. In Chrome, open `chrome://extensions` and enable **Developer mode**.
3. Click **Load unpacked** and select this folder.
4. Pin the Strata extension and check that the popup says **Connected to Strata**.

Leave **Capture downloads** on to send eligible HTTP/HTTPS file downloads to the app. Turn it off to keep downloads in Chrome. You can also paste a link into the popup or right-click a link and choose **Download with Strata**.

The extension pauses Chrome's copy while it asks Strata to accept the file. Chrome's copy is canceled only after the app confirms acceptance. If the bridge or app is unavailable, the browser download resumes.

Private downloads, browser-generated files, and files flagged by Chrome remain in Chrome. Cookies and passwords are not exported. Login-only or POST-generated files may need capture turned off. Errors after an accepted handoff appear in Strata.

The desktop app and `strata-bridge.exe` must remain beside each other. If you move them, reconnect the bridge. The extension ID stays fixed through the public key in `manifest.json`.

To disconnect, turn capture off and remove the extension in Chrome. `install-browser.ps1 -Uninstall` unregisters the app connection without deleting downloads or history.
