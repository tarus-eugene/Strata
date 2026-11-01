# 101 Downloader

A native Windows download manager written in Rust and GPUI. A calm dark queue with parallel HTTP transfers, FTP, SFTP, WebDAV, native torrents, verified Metalinks, and video downloads through yt-dlp.

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

Source builds retain the crate's `strata.exe`, `strata-cli.exe`, and `strata-bridge.exe` names; the installer names them `101Downloader.exe`, `101Downloader-cli.exe`, and `101Downloader-Bridge.exe`. The release executable is `target\release\strata.exe`. Keep `Cargo.lock` to reproduce the resolved dependency versions. This project was built with Rust 1.92.0.

## Using 101 Downloader

- Add a URL or magnet with **New download** or **Ctrl+N**. The centered modal fills a valid clipboard source. **Open torrent / Metalink file** selects a descriptor. Enter starts; Escape or clicking outside closes it. Tab stays within the modal controls. Native text inputs use a blinking caret at the Windows caret rate.
- Search with **Ctrl+F**. The full-width table shows file name, size, status, progress, speed, and ETA. Unknown-size and stopped transfers show no ETA. Summary cards are omitted to reclaim table space. The compact toolbar centers search, groups **Filter**, **Settings**, and **New download** on the right, and shows selection actions on the left. With nothing selected, only **Pause all** appears there.
- Click a row to inspect its destination, source, completion date/time, priority, speed limit, and segmented connection-progress bar in the bottom panel. The panel fits without scrolling. Drag its top divider to resize it; focus the divider and press **Up/Down** for keyboard resizing. Its height is remembered. Use the down arrow in the footer at the bottom of the window to collapse the panel, and the up arrow to restore it.
- Each row starts with a checkbox. **Ctrl+click** toggles individual selections, **Shift+click** selects a range in the current displayed order, and **Ctrl+Shift+click** adds a range. The header checkbox selects or clears the visible rows; **Ctrl+A** selects them when the table has focus. Pause, resume, cancel, delete, speed-limit, and priority context actions apply to the selection. Right-click a row or use its three-dot menu.
- **Rename** opens a modal with the current name selected except for the last file extension. Folder names are selected in full. Active transfers pause briefly and resume after renaming; completed files/folders are moved without replacing existing paths. Invalid names and collisions leave the original download intact.
- **Speed limit** in a row's context menu sets a limit for each selected download, shared by all of its connections. **Settings → Global speed limit** sets a shared cap across all downloads. Values are in KiB/s; **0** means unlimited. Global and torrent limits start at 16 KiB/s. Under a global cap, FTP, video, torrent, and Metalink jobs occupy the queue exclusively so their separate rate controls cannot exceed that cap. HTTP and SFTP jobs share it concurrently. Changing limits pauses affected active transfers briefly, then resumes them.
- **Set priority → High / Normal / Low** controls which queued transfer starts next. Equal priorities keep creation order; active transfers finish without preemption. Priority, limits, and renamed destinations survive restart.
- The **Created** column uses the Windows short date format. **Completed at** uses the Windows local date and preferred 12/24-hour time, without seconds. Older completed entries without a recorded completion timestamp show a dash.
- Each row has a **three-dot menu**. **Delete from device** waits for writes to stop, permanently deletes its owned file/output folder and partial data, then removes the entry. Locked-file and permission failures keep the entry with an error so deletion can be retried. **Cancel** retains partial data.
- Pause/resume individual downloads, pause all, or cancel a transfer. **Closing the window hides 101 Downloader to the system tray and keeps downloads running.** Click the tray icon or choose **Open 101 Downloader** from its right-click menu to restore it. Launching the app again also restores its existing window.
- To fully exit, right-click the tray icon and choose **Quit 101 Downloader**. Quit pauses active transfers and saves history; they reopen paused next time. If Windows puts the tray icon in the overflow area, use the taskbar's hidden-icons arrow to find it.
- Choose the download folder and 1, 4, 8, or 16 connections in Settings. Up to three downloads run simultaneously.
- The app checks this repository's public GitHub Releases at startup and offers **Download update** when a newer stable Windows version is available. Use **Settings → App updates → Check for updates** to check again or retry a failed check. **Download update** adds the installer to your normal download queue. After it finishes, quit through the tray menu and run the installer; history, settings, and downloaded files are retained. The app never downloads or runs an installer automatically.
- Open a finished file or its folder from the row's context menu. 101 Downloader never automatically opens downloaded files.
- The custom title bar supports dragging, double-click maximize/restore, window snapping, minimize, and close. Its controls also work with keyboard focus.

The tray icon is removed on quit and re-added after Windows Explorer restarts. If the notification area is unavailable, a visible message explains that closing will quit instead of hiding the only reachable window.

## Windows installer

Run `dist\101-Downloader-Setup-0.3.2-x64.exe` to install 101 Downloader for your Windows account. Administrator access is not required. It includes the optimized app, `101Downloader-cli.exe`, local browser bridge, Visual C++ runtime DLLs, browser extension, pinned yt-dlp / FFmpeg / Deno helpers, Start menu shortcuts, and an optional desktop shortcut. Windows 10 version 2004 or later, Windows 11, and a working GPU driver are required.

The browser connection task is selected by default. The extension itself must be loaded once in each browser profile; the installer does not change browser security policies. Use the Start menu's **Browser companion setup** shortcut for instructions. Uninstall from Windows Settings → Apps; downloaded files, partial transfers, history, and settings are retained. Quit 101 Downloader through its tray menu before upgrading or uninstalling.

To rebuild the installer, install [Inno Setup](https://jrsoftware.org/isdl.php) 6.3 or later and the Visual Studio C++ build tools, then run:

```powershell
.\scripts\package-installer.ps1
```

This builds the release executables and writes a versioned setup EXE, SHA-256 file, and `101-downloader-browser-extension.zip` to `dist`. Use `-SkipBuild` to package an existing release build, `-CompilerPath` for a custom `ISCC.exe`, or `-RuntimeDirectory` for the x64 `Microsoft.VC*.CRT` redistributable folder. Build staging stays under the ignored `.tools` directory. No Rust toolchain is needed on the computer where 101 Downloader is installed. The setup EXE is currently unsigned.

Media helpers are downloaded from their official release repositories and checked against the sizes and SHA-256 hashes in `resources/media-tools.json`. The installed `tools` folder includes their licenses and source/build links. Use `scripts/fetch-media-tools.ps1` to prepare the development cache; source debug builds also discover that cache. Media helpers can alternatively be placed beside the app in `tools` or on PATH. The first installer build needs network access to fetch these assets.

## Protocols and command line

The GUI detects protocols from URLs and descriptor filenames. `101Downloader-cli.exe` accepts the same sources and adds explicit protocol flags. It queues the transfer in the desktop app, starting the app if necessary; it does not launch a separate downloader queue.

| Protocol | Detection / override | Behavior |
| --- | --- | --- |
| HTTP / HTTPS | Automatic | Parallel ranges, safe resume, original file bytes; HTTP, HTTPS, SOCKS5 and SOCKS5h proxies |
| FTP | `ftp://` or `--ftp` | Anonymous or authenticated binary transfer; REST resume |
| SFTP | `sftp://` or `--sftp` | Password or OpenSSH private key; strict known-host verification; seek resume |
| WebDAV | `webdav://`, `webdavs://` or `--webdav` | HTTP(S) GET with URL-provided Basic authentication; normal HTTP range behavior |
| BitTorrent | Magnet or `.torrent` file / URL | Native `librqbit` through the `torrent-native` Cargo feature; v1 / hybrid torrents, DHT, trackers, piece checks and partial-file rechecking |
| Metalink | `.meta4`, `.metalink` or `--metalink` | v3 and v4; ordered mirror failover, SHA-256 / SHA-512 / MD5 whole-file checks, multiple files |
| Video sites | Known video host or `--ytdlp` | yt-dlp extraction; FFmpeg merges and Deno supports JavaScript challenges |

```powershell
.\101Downloader-cli.exe https://example.com/large.zip --connections 8
.\101Downloader-cli.exe https://example.com/large.zip --proxy socks5h://localhost:1080
.\101Downloader-cli.exe --ftp ftp.example.com/pub/file.zip
.\101Downloader-cli.exe --sftp alice@example.com/archive.zip --identity C:\Users\alice\.ssh\id_ed25519
.\101Downloader-cli.exe --webdav https://alice:password@example.com/archive.zip
.\101Downloader-cli.exe 'magnet:?xt=urn:btih:YOUR_INFO_HASH'
.\101Downloader-cli.exe C:\Downloads\release.torrent
.\101Downloader-cli.exe --metalink https://example.com/release.meta4
.\101Downloader-cli.exe --ytdlp https://www.youtube.com/watch?v=VIDEO_ID
```

Installed executables are under `%LOCALAPPDATA%\Programs\101 Downloader` by default. Source builds use `target\debug` or `target\release`. `101Downloader.exe` also accepts the transfer flags; the console CLI provides visible command-line errors and queue IDs.

Use `--user`, `--password-stdin`, `--identity`, `--key-passphrase-stdin`, `--known-hosts`, `--directory`, and `--connections` for explicit options. For a password without putting it in shell history:

```powershell
$credential = Get-Credential -UserName alice
$credential.GetNetworkCredential().Password | .\101Downloader-cli.exe sftp://alice@example.com/archive.zip --password-stdin
```

URL passwords, private-key passphrases, and proxy passwords are protected with Windows DPAPI for your current Windows account before history is saved. Source links omit user information. These credentials cannot be decrypted by another Windows account; re-enter them after moving history to a different account or machine. URL query parameters are retained as part of the download link.

SFTP uses `%USERPROFILE%\.ssh\known_hosts` by default and refuses unknown, changed, or revoked keys. Verify the host fingerprint with the server administrator, connect with OpenSSH to record the verified key, or supply `--known-hosts`. Settings can select a private key / known-hosts file and configure the HTTP proxy. Explicitly pinned host keys are supported; CA-only known-hosts entries do not authorize an unpinned key. Encrypted private keys accept `--key-passphrase-stdin`.

Torrent, Metalink, and video jobs publish a download folder. They stage files privately and do not replace an existing destination. Torrents first load metadata, then open a file tree before downloading payloads. Expand folders to choose individual files, or use folder checkboxes to select their contents; partial selections show a mixed checkbox. The dialog shows the selected count and size. Cancel leaves the torrent paused; Resume reopens selection. Choices persist across restarts, and completed output includes only selected files. This also applies to magnets and browser/CLI handoffs. Native torrents upload while active and stop seeding on completion or pause. Resume rechecks existing pieces. Metalink descriptors are limited to 8 MiB / 1024 files, reject unsafe paths and XML DTDs, and require a supported whole-file checksum for each file. A bad mirror or checksum is tried against the next mirror; no unverified file is published.

Video hosts automatically recognized include YouTube, Vimeo, Twitch, TikTok, and Instagram, including their subdomains; `--ytdlp` selects the extractor for other supported sites. Site changes, login requirements, DRM, and availability can prevent an extraction. Browser cookies are not exported automatically. Pause stops the media process tree and retains resumable files.

The `torrent-native` feature is enabled by default and included in the installer. To omit it, run `.\run.ps1 build -CargoArguments @('--no-default-features')`. `run.ps1` forwards `-CargoArguments` to Cargo for targeted tests and feature checks.

## Chromium browser companion

The same extension supports Windows Chromium browsers with Manifest V3, downloads, and native messaging enabled: Chrome, Edge, Brave, Vivaldi, Opera / Opera GX, Chromium, and compatible forks. Connect once using the installer or **Settings → Browser companion → Connect browsers**. From a source checkout, run `.\install-browser.ps1`.

1. Open your browser's extensions page (`chrome://extensions`, `edge://extensions`, `brave://extensions`, `vivaldi://extensions`, or `opera://extensions`) and turn on **Developer mode**.
2. Choose **Load unpacked** and select the `extension` folder inside your 101 Downloader installation or source checkout. **Settings → Extension folder** opens the correct location.
3. Pin **101 Downloader — Download companion**. Open its popup to check the app connection and turn capture on or off.

When building from source, build both executables with `.\run.ps1 build`, then use **Settings → Browser companion → Connect browsers**, or run:

```powershell
.\install-browser.ps1
```

Keep the installed `101Downloader.exe` and `101Downloader-Bridge.exe` in the same folder (source builds retain `strata.exe` and `strata-bridge.exe`). If you move the executables, reconnect the bridge. For a release build, run `.\install-browser.ps1 -AppDirectory .\target\release`. Unregister it with `.\install-browser.ps1 -Uninstall`; this retains your downloads and history. The installer removes its own registration during uninstall and preserves a registration that has since been pointed to a different 101 Downloader installation.

With capture enabled, the extension pauses eligible HTTP/HTTPS file downloads, sends the URL, browser-provided filename, and referrer to 101 Downloader, and cancels Chrome's paused copy **only after the app confirms the download was saved to its queue**. If the native host is unavailable or the app refuses the request, Chrome resumes its own download. A suspended extension worker recovers its pending handoffs on restart.

Capture is enabled by default: start a normal browser download and it is sent to 101 Downloader automatically, with no **Download with 101 Downloader** action required. Accepted browser downloads open or restore the app, including when it is hidden in the tray. Downloads awaiting browser scanning are retried when the browser reports them safe. The context-menu command and popup URL field remain optional shortcuts. Checking connection status alone does not open the app.

The extension uses Manifest V3 with `downloads`, `downloads.ui`, `nativeMessaging`, `storage`, and `contextMenus` permissions. No page injection, remote server, browsing-history access, cookies, or passwords. Browser-generated `blob:` files, private downloads, already-paused downloads, and files flagged by Chrome remain in the browser. Login-only and POST-generated downloads may require keeping capture off. A queue acknowledgement confirms handoff; later network failures are shown in 101 Downloader.

The bridge uses Chrome's [native messaging protocol](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging) and a local named pipe restricted to your Windows user and SYSTEM. The committed extension public key keeps its ID stable: `eaccbjfepcpidchboonodedpepgboolc`. Only that extension origin is allowed; messages are length-bounded and accept fixed actions, never commands.

The bridge is registered in the per-user Chrome, Chromium, and Edge native messaging locations. [Edge documents its own location and the Chromium/Chrome fallbacks](https://learn.microsoft.com/en-us/microsoft-edge/extensions-chromium/developer-guide/native-messaging); [Opera documents the Chrome registration location](https://help.opera.com/en/extensions/message-passing/). Other compatible Chromium forks use their standard native messaging lookup. Fork-specific restrictions and managed browser policies can prevent connection; every fork has not been tested individually. This desktop bridge targets Windows, not Android or iOS.

## Logo

The logo uses the user-supplied orange hanging orangutan illustration, with its original black background and colors. The palette uses branding orange #E85002, black #000000, white #F9F9F9, and neutral grays #A7A7A7, #646464, and #333333 from the supplied reference. The unchanged source image is `assets/brand/mark.png`. The app embeds that PNG and a Windows ICO; the extension ships matching resized PNGs. Regenerate the platform icons with `.\scripts\generate-icons.ps1`.

## How parallel downloading works

1. Probe with `Range: bytes=0-0` and `Accept-Encoding: identity`.
2. Require a valid `206 Content-Range`, a known size, and a strong ETag or Last-Modified validator before segmenting. Use fewer connections for small files (minimum target part size: 256 KiB).
3. Put non-overlapping ranges into a shared per-download queue (target 4 MiB, at least the configured worker count). A fixed pool of workers pulls the next chunk and reuses pooled HTTP connections; the configured connection limit still applies. Each chunk has its own part file.
4. Validate every range response, total size, encoding, and any returned validator. Retry a broken connection up to three times, continuing from the part's actual length. When the queue empties and workers are idle, split a slow active range's remaining bytes. Stop and flush its writer before shortening the range and queuing the new tail; saved part indices stay stable.
5. For resume, probe again and compare the saved validator and size. Discard stale part contents if the remote file changed. Send `If-Range` with range requests.
6. Assemble complete parts in byte order, flush the file, and publish it without replacing an existing destination. Remove this transfer's parts after success.

Servers that ignore ranges, omit validators, or lack a known size use a single full-file connection. Those transfers restart from zero on resume. This deliberately favors file integrity over pretending every server supports resumable parallel downloads. Parallel connections help on some servers; they cannot increase the bandwidth of your internet connection.

## Text clarity

GPUI 0.2.2's Windows DirectWrite backend requests **grayscale** antialiasing; toggling ClearType does not turn it into a subpixel renderer. See [the pinned DirectWrite source](https://docs.rs/crate/gpui/0.2.2/source/src/platform/windows/direct_write.rs). Upstream also tracks [additional hinting support](https://github.com/zed-industries/zed/issues/55214).

101 Downloader embeds GPUI's **PerMonitorV2** manifest, uses native Segoe UI and opaque surfaces, and paints labels and editable text on **physical-pixel-aligned baselines**. `src/text.rs` wraps GPUI's normal text layout, preserving shaping, wrapping, glyph advances, and font size. Only the paint origin is adjusted; there is no texture scaling or global Windows setting change. A unit test checks alignment at 100%, 125%, 150%, 175%, and 200% scaling.

These changes address avoidable scaling and baseline softness. They do not replace GPUI's grayscale rasterizer or guarantee identical appearance on every monitor and GPU.

## Storage and scope

History and settings: `%LOCALAPPDATA%\101 Downloader\history.json` for new installs. Existing `%LOCALAPPDATA%\Strata\history.json` continues to be used when present, preserving downloads, settings and encrypted credentials through the rebrand. Partial files: `.strata-<id>.parts` inside each download's destination. Writes to history are atomic. The app holds a process lock to prevent two instances from editing the same history.

For portable storage or isolated testing, set `DOWNLOADER101_DATA_DIR` (the legacy `STRATA_DATA_DIR` alias still works) to a directory before launching. It changes the history/settings location, not the destination of existing downloads.

Authenticated browser-cookie import and scheduling are outside this version. Filename recovery uses browser filename determination, Content-Disposition (including UTF-8 filename*), readable redirect URLs, and MIME extensions for temporary names. Later transfer headers are checked again before publishing the file. Meaningful browser-provided names and Metalink entry paths are retained, and existing files are never replaced. If neither browser nor server supplies an original name, that name cannot be reconstructed from arbitrary file bytes.

## Checks

The local HTTP test server verifies simultaneous range requests and byte-for-byte assembly, ordinary full-file fallback, empty files, pause/resume across manager restart, remote file changes, malformed range rejection, collision protection, filename sanitation, and range partitioning. It never downloads test files from the public internet.

Loopback transport tests verify WebDAV authentication, authenticated SOCKS5 with remote DNS, HTTP proxy routing, FTP anonymous/password authentication and REST resume, SFTP password/key authentication and resume, and Metalink mirror recovery after checksum failure. A real local BitTorrent seed verifies `.torrent` and magnet transfers and rechecking existing partial data. Host-key tests reject unknown, changed, and revoked keys. Credential tests check protected persistence and diagnostic redaction. The optional `ytdlp_download_pause_and_resume_are_byte_exact` test uses the pinned helper against a local media fixture; run it with `.\run.ps1 test -CargoArguments @('--test','downloads','ytdlp_download_pause_and_resume_are_byte_exact','--','--ignored','--exact')`.

`node --test extension/background.test.js` checks capture ordering, failure recovery, capture-off behavior, private/unsafe URL exclusions, cancellation failures, and worker restart recovery. Rust also checks native-message framing, named-pipe handoff, filename/referrer forwarding, and rejecting a transfer when history cannot be persisted.

Tray checks cover mouse and keyboard callback routing. The ignored `native_tray_hides_restores_and_cleans_up` test requires interactive Windows Explorer; it creates its own window and verifies native tray creation, restore, hide without destroying the window, and cleanup. It was also run successfully on Windows.

Earlier 0.1.x end-to-end verification used isolated Chromium and Brave profiles: a real browser download was handed through the native host to the app, the browser's copy was canceled after acceptance, and the app's 4 MiB file matched every source byte. With capture off, the next file completed in the browser without creating an app transfer. The popup rendered its live connected state without script errors. Brave was checked against the optimized release app.

`scripts/test-installer.ps1` exercises the real setup EXE in a directory with spaces: running-app protection, install, repeat-install upgrade, runtime and extension payloads, Start menu shortcut, native host registrations, and uninstall. It starts the installed CLI and each media helper with a restricted PATH, checks that unrelated data remains, and verifies that a bridge registration belonging to a different installation survives. Use a Windows account without an existing installed Strata / 101 Downloader or either product's Start menu shortcuts; quit any running copy first. The test snapshots and restores existing native host registrations and leaves diagnostic logs under `.tools/installer-test`.

The editable text input is adapted from GPUI's Apache-2.0 example; see `NOTICE` and `LICENSE-APACHE`.

Browser download-popup suppression follows the capture switch using the Chromium `downloads.ui` API. Reload the updated companion and approve its download-interface permission if requested. Disabling capture, handing a transfer back to the browser, or encountering a browser-only download restores the UI. The API controls all windows in the current browser profile; a simultaneous browser-only transfer can re-enable that UI. Forks without the API still hand off downloads.

Filename and deletion regressions cover encoded and quoted server names, late headers, redirects, extension fallback, collisions, resume, Metalink paths, permanent cleanup, locked-file retry, interrupted deletion, and rejecting paths outside the recorded output directory. Caret and ETA tests cover blink timing and unknown-size/stalled transfers.
The chunk-queue tests verify keep-alive TCP reuse with more chunks than workers, the connection limit, slow-tail splitting, and byte-exact resume after a split. The 0.2.0 native handoff is smoke-tested separately; current browser popup visuals and a full installer upgrade/uninstall cycle still need an interactive Windows check.

Scrollable tables, Settings, and overflowing dialogs show draggable scrollbars. Mouse-wheel scrolling eases between positions; precise pixel scrolling stays direct. Windows animation preferences are respected. Settings keeps an orange border on its toolbar button while open.
