# Strata

A native Windows download manager written in Rust and GPUI. A calm dark queue, live transfer details, and real parallel HTTP downloads.

## Run

```powershell
.\run.ps1
```

Or open `target\debug\strata.exe` after building. The executable runs independently of Cargo; no web server or browser is required.

The script uses the project-local Rust installation when present and finds the Visual Studio C++ environment automatically. On a fresh machine, install stable [Rust](https://rustup.rs/), Visual Studio C++ Build Tools, and a Windows SDK. GPUI requires a working GPU driver.

```powershell
.\run.ps1 build
.\run.ps1 test
.\run.ps1 clippy
.\run.ps1 build -Release
```

The release executable is `target\release\strata.exe`. Keep `Cargo.lock` to reproduce the resolved dependency versions. This project was built with Rust 1.92.0.

## Using Strata

- Add an HTTP or HTTPS file link with **New download** or **Ctrl+N**. A valid link on the clipboard is filled in automatically. Press **Enter** to start.
- Search with **Ctrl+F**. Select a row to inspect its individual connection lanes, progress, destination, and source.
- Pause/resume individual downloads, pause all, or cancel a transfer. **Closing the window hides Strata to the system tray and keeps downloads running.** Click the tray icon or choose **Open Strata** from its right-click menu to restore it. Launching the app again also restores its existing window.
- To fully exit, right-click the tray icon and choose **Quit Strata**. Quit pauses active transfers and saves history; they reopen paused next time. If Windows puts the tray icon in the overflow area, use the taskbar's hidden-icons arrow to find it.
- Choose the download folder and 1, 4, 8, or 16 connections in Settings. Up to three downloads run simultaneously.
- Open a finished file or its folder from the inspector. Strata never automatically opens downloaded files.
- The custom title bar supports dragging, double-click maximize/restore, window snapping, minimize, and close. Its controls also work with keyboard focus.

The tray icon is removed on quit and re-added after Windows Explorer restarts. If the notification area is unavailable, a visible message explains that closing will quit instead of hiding the only reachable window.

## Windows installer

Run `dist\Strata-Setup-0.1.0-x64.exe` to install Strata for your Windows account. Administrator access is not required. It includes the optimized app, local browser bridge, Visual C++ runtime DLLs, browser extension, Start menu shortcuts, and an optional desktop shortcut. Windows 10 version 2004 or later, Windows 11, and a working GPU driver are required.

The browser connection task is selected by default. The extension itself must be loaded once in each browser profile; the installer does not change browser security policies. Use the Start menu's **Browser companion setup** shortcut for instructions. Uninstall from Windows Settings → Apps; downloaded files, partial transfers, history, and settings are retained. Quit Strata through its tray menu before upgrading or uninstalling.

To rebuild the installer, install [Inno Setup](https://jrsoftware.org/isdl.php) 6.3 or later and the Visual Studio C++ build tools, then run:

```powershell
.\scripts\package-installer.ps1
```

This builds both release executables and writes a versioned setup EXE, SHA-256 file, and `strata-browser-extension.zip` to `dist`. Use `-SkipBuild` to package an existing release build, `-CompilerPath` for a custom `ISCC.exe`, or `-RuntimeDirectory` for the x64 `Microsoft.VC*.CRT` redistributable folder. Build staging stays under the ignored `.tools` directory. No Rust toolchain is needed on the computer where Strata is installed. The setup EXE is currently unsigned.

## Chromium browser companion

The same extension supports Windows Chromium browsers with Manifest V3, downloads, and native messaging enabled: Chrome, Edge, Brave, Vivaldi, Opera / Opera GX, Chromium, and compatible forks. Connect once using the installer or **Settings → Browser companion → Connect browsers**. From a source checkout, run `.\install-browser.ps1`.

1. Open your browser's extensions page (`chrome://extensions`, `edge://extensions`, `brave://extensions`, `vivaldi://extensions`, or `opera://extensions`) and turn on **Developer mode**.
2. Choose **Load unpacked** and select the `extension` folder inside your Strata installation or source checkout. **Settings → Extension folder** opens the correct location.
3. Pin **Strata — Download companion**. Open its popup to check the app connection and turn capture on or off.

When building from source, build both executables with `.\run.ps1 build`, then use **Settings → Browser companion → Connect browsers**, or run:

```powershell
.\install-browser.ps1
```

Keep `strata.exe` and `strata-bridge.exe` in the same folder. If you move the executables, reconnect the bridge. For a release build, run `.\install-browser.ps1 -AppDirectory .\target\release`. Unregister it with `.\install-browser.ps1 -Uninstall`; this retains your downloads and history. The installer removes its own registration during uninstall and preserves a registration that has since been pointed to a different Strata installation.

With capture enabled, the extension pauses eligible HTTP/HTTPS file downloads, sends the URL, browser-provided filename, and referrer to Strata, and cancels Chrome's paused copy **only after the app confirms the download was saved to its queue**. If the native host is unavailable or the app refuses the request, Chrome resumes its own download. A suspended extension worker recovers its pending handoffs on restart.

You can also right-click a file link and choose **Download with Strata**, or paste a URL into the popup. The app opens automatically for a download if it is closed. Checking connection status alone does not open it.

The extension uses Manifest V3 with only `downloads`, `nativeMessaging`, `storage`, and `contextMenus` permissions. No page injection, remote server, browsing-history access, cookies, or passwords. Browser-generated `blob:` files, private downloads, already-paused downloads, and files flagged by Chrome remain in the browser. Login-only and POST-generated downloads may require keeping capture off. A queue acknowledgement confirms handoff; later network failures are shown in Strata.

The bridge uses Chrome's [native messaging protocol](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging) and a local named pipe restricted to your Windows user and SYSTEM. The committed extension public key keeps its ID stable: `eaccbjfepcpidchboonodedpepgboolc`. Only that extension origin is allowed; messages are length-bounded and accept fixed actions, never commands.

The bridge is registered in the per-user Chrome, Chromium, and Edge native messaging locations. [Edge documents its own location and the Chromium/Chrome fallbacks](https://learn.microsoft.com/en-us/microsoft-edge/extensions-chromium/developer-guide/native-messaging); [Opera documents the Chrome registration location](https://help.opera.com/en/extensions/message-passing/). Other compatible Chromium forks use their standard native messaging lookup. Fork-specific restrictions and managed browser policies can prevent connection; every fork has not been tested individually. This desktop bridge targets Windows, not Android or iOS.

## Logo

The layered downward mark represents parallel transfers arriving as one file. Editable artwork: `assets/brand/mark.svg` and `assets/brand/wordmark.svg`. The app embeds the SVG and Windows ICO; the extension ships matching PNGs. Regenerate the platform icons with `.\scripts\generate-icons.ps1`.

## How parallel downloading works

1. Probe with `Range: bytes=0-0` and `Accept-Encoding: identity`.
2. Require a valid `206 Content-Range`, a known size, and a strong ETag or Last-Modified validator before segmenting. Use fewer connections for small files (minimum target part size: 256 KiB).
3. Divide the file into non-overlapping inclusive ranges. Each background task writes to its own part file. The interface remains on GPUI's foreground thread.
4. Validate every range response, total size, encoding, and any returned validator. Retry a broken connection up to three times, continuing from the part's actual length.
5. For resume, probe again and compare the saved validator and size. Discard stale part contents if the remote file changed. Send `If-Range` with range requests.
6. Assemble complete parts in order, flush the file, and publish it without replacing an existing destination. Remove this transfer's parts after success.

Servers that ignore ranges, omit validators, or lack a known size use a single full-file connection. Those transfers restart from zero on resume. This deliberately favors file integrity over pretending every server supports resumable parallel downloads. Parallel connections help on some servers; they cannot increase the bandwidth of your internet connection.

## Text clarity

GPUI 0.2.2's Windows DirectWrite backend requests **grayscale** antialiasing; toggling ClearType does not turn it into a subpixel renderer. See [the pinned DirectWrite source](https://docs.rs/crate/gpui/0.2.2/source/src/platform/windows/direct_write.rs). Upstream also tracks [additional hinting support](https://github.com/zed-industries/zed/issues/55214).

Strata embeds GPUI's **PerMonitorV2** manifest, uses native Segoe UI and opaque surfaces, and paints labels and editable text on **physical-pixel-aligned baselines**. `src/text.rs` wraps GPUI's normal text layout, preserving shaping, wrapping, glyph advances, and font size. Only the paint origin is adjusted; there is no texture scaling or global Windows setting change. A unit test checks alignment at 100%, 125%, 150%, 175%, and 200% scaling.

These changes address avoidable scaling and baseline softness. They do not replace GPUI's grayscale rasterizer or guarantee identical appearance on every monitor and GPU.

## Storage and scope

History and settings: `%LOCALAPPDATA%\Strata\history.json`. Partial files: `.strata-<id>.parts` inside each download's destination. Writes to history are atomic. The app holds a process lock to prevent two instances from editing the same history.

For portable storage or isolated testing, set `STRATA_DATA_DIR` to a directory before launching. It changes the history/settings location, not the destination of existing downloads.

This version handles direct file links and Chrome capture. Authenticated cookie import, video extraction, scheduling, speed limits, and dynamic redistribution of a slow connection's remaining range are outside its scope. Names come from Chrome when provided, or from the URL path; Content-Disposition names are not yet read directly by the app. Cancellation stops the transfer and retains its partial files; remove the matching hidden parts folder manually if you no longer need it.

## Checks

The local HTTP test server verifies simultaneous range requests and byte-for-byte assembly, ordinary full-file fallback, empty files, pause/resume across manager restart, remote file changes, malformed range rejection, collision protection, filename sanitation, and range partitioning. It never downloads test files from the public internet.

`node --test extension/background.test.js` checks capture ordering, failure recovery, capture-off behavior, private/unsafe URL exclusions, cancellation failures, and worker restart recovery. Rust also checks native-message framing, named-pipe handoff, filename/referrer forwarding, and rejecting a transfer when history cannot be persisted.

Tray checks cover mouse and keyboard callback routing. The ignored `native_tray_hides_restores_and_cleans_up` test requires interactive Windows Explorer; it creates its own window and verifies native tray creation, restore, hide without destroying the window, and cleanup. It was also run successfully on Windows.

End-to-end verification used isolated Chromium and Brave profiles: a real browser download was handed through the native host to the app, the browser's copy was canceled after acceptance, and the app's 4 MiB file matched every source byte. With capture off, the next file completed in the browser without creating an app transfer. The popup rendered its live connected state without script errors. Brave was checked against the optimized release app.

`scripts/test-installer.ps1` exercises the real setup EXE in a directory with spaces: running-app protection, install, repeat-install upgrade, runtime and extension payloads, Start menu shortcut, native host registrations, and uninstall. It checks that unrelated data remains and a bridge registration belonging to a different installation survives. Use a Windows account without an existing installed Strata or Strata Start menu shortcut; quit any running copy first. The test snapshots and restores existing native host registrations and leaves diagnostic logs under `.tools/installer-test`.

The editable text input is adapted from GPUI's Apache-2.0 example; see `NOTICE` and `LICENSE-APACHE`.
