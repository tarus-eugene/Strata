# Strata

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

The release executable is `target\release\strata.exe`. Keep `Cargo.lock` to reproduce the resolved dependency versions. This project was built with Rust 1.92.0.

## Using Strata

- Add a URL or magnet with **New download** or **Ctrl+N**. A valid source on the clipboard is filled in automatically. **Open torrent / Metalink file** selects a local descriptor. Press **Enter** to start.
- Search with **Ctrl+F**. The full-width download table shows file name, size, status, progress, and speed. Use **Filter** at the top for All downloads, In progress, Completed, or Stopped; **Settings** is at the top right.
- Click a row to inspect its destination, source, and one segmented connection-progress bar in the bottom panel. Drag the panel's top divider to resize it; focus the divider and press **Up/Down** for keyboard resizing. Its height is remembered.
- Each row's **three-dot menu** contains its available pause/resume, open, open-folder, copy-link, cancel, and delete actions. **Delete** stops the transfer and removes its history entry while keeping downloaded files and partial data.
- Pause/resume individual downloads, pause all, or cancel a transfer. **Closing the window hides Strata to the system tray and keeps downloads running.** Click the tray icon or choose **Open Strata** from its right-click menu to restore it. Launching the app again also restores its existing window.
- To fully exit, right-click the tray icon and choose **Quit Strata**. Quit pauses active transfers and saves history; they reopen paused next time. If Windows puts the tray icon in the overflow area, use the taskbar's hidden-icons arrow to find it.
- Choose the download folder and 1, 4, 8, or 16 connections in Settings. Up to three downloads run simultaneously.
- Open a finished file or its folder from the inspector. Strata never automatically opens downloaded files.
- The custom title bar supports dragging, double-click maximize/restore, window snapping, minimize, and close. Its controls also work with keyboard focus.

The tray icon is removed on quit and re-added after Windows Explorer restarts. If the notification area is unavailable, a visible message explains that closing will quit instead of hiding the only reachable window.

## Windows installer

Run `dist\Strata-Setup-0.1.3-x64.exe` to install Strata for your Windows account. Administrator access is not required. It includes the optimized app, `strata-cli.exe`, local browser bridge, Visual C++ runtime DLLs, browser extension, pinned yt-dlp / FFmpeg / Deno helpers, Start menu shortcuts, and an optional desktop shortcut. Windows 10 version 2004 or later, Windows 11, and a working GPU driver are required.

The browser connection task is selected by default. The extension itself must be loaded once in each browser profile; the installer does not change browser security policies. Use the Start menu's **Browser companion setup** shortcut for instructions. Uninstall from Windows Settings → Apps; downloaded files, partial transfers, history, and settings are retained. Quit Strata through its tray menu before upgrading or uninstalling.

To rebuild the installer, install [Inno Setup](https://jrsoftware.org/isdl.php) 6.3 or later and the Visual Studio C++ build tools, then run:

```powershell
.\scripts\package-installer.ps1
```

This builds the release executables and writes a versioned setup EXE, SHA-256 file, and `strata-browser-extension.zip` to `dist`. Use `-SkipBuild` to package an existing release build, `-CompilerPath` for a custom `ISCC.exe`, or `-RuntimeDirectory` for the x64 `Microsoft.VC*.CRT` redistributable folder. Build staging stays under the ignored `.tools` directory. No Rust toolchain is needed on the computer where Strata is installed. The setup EXE is currently unsigned.

Media helpers are downloaded from their official release repositories and checked against the sizes and SHA-256 hashes in `resources/media-tools.json`. The installed `tools` folder includes their licenses and source/build links. Use `scripts/fetch-media-tools.ps1` to prepare the development cache; source debug builds also discover that cache. Media helpers can alternatively be placed beside the app in `tools` or on PATH. The first installer build needs network access to fetch these assets.

## Protocols and command line

The GUI detects protocols from URLs and descriptor filenames. `strata-cli.exe` accepts the same sources and adds explicit protocol flags. It queues the transfer in the desktop app, starting the app if necessary; it does not launch a separate downloader queue.

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
.\strata-cli.exe https://example.com/large.zip --connections 8
.\strata-cli.exe https://example.com/large.zip --proxy socks5h://localhost:1080
.\strata-cli.exe --ftp ftp.example.com/pub/file.zip
.\strata-cli.exe --sftp alice@example.com/archive.zip --identity C:\Users\alice\.ssh\id_ed25519
.\strata-cli.exe --webdav https://alice:password@example.com/archive.zip
.\strata-cli.exe 'magnet:?xt=urn:btih:YOUR_INFO_HASH'
.\strata-cli.exe C:\Downloads\release.torrent
.\strata-cli.exe --metalink https://example.com/release.meta4
.\strata-cli.exe --ytdlp https://www.youtube.com/watch?v=VIDEO_ID
```

Installed executables are under `%LOCALAPPDATA%\Programs\Strata` by default. Source builds use `target\debug` or `target\release`. `strata.exe` also accepts the transfer flags; the console CLI provides visible command-line errors and queue IDs.

Use `--user`, `--password-stdin`, `--identity`, `--key-passphrase-stdin`, `--known-hosts`, `--directory`, and `--connections` for explicit options. For a password without putting it in shell history:

```powershell
$credential = Get-Credential -UserName alice
$credential.GetNetworkCredential().Password | .\strata-cli.exe sftp://alice@example.com/archive.zip --password-stdin
```

URL passwords, private-key passphrases, and proxy passwords are protected with Windows DPAPI for your current Windows account before history is saved. Source links omit user information. These credentials cannot be decrypted by another Windows account; re-enter them after moving history to a different account or machine. URL query parameters are retained as part of the download link.

SFTP uses `%USERPROFILE%\.ssh\known_hosts` by default and refuses unknown, changed, or revoked keys. Verify the host fingerprint with the server administrator, connect with OpenSSH to record the verified key, or supply `--known-hosts`. Settings can select a private key / known-hosts file and configure the HTTP proxy. Explicitly pinned host keys are supported; CA-only known-hosts entries do not authorize an unpinned key. Encrypted private keys accept `--key-passphrase-stdin`.

Torrent, Metalink, and video jobs publish a download folder. They stage files privately and do not replace an existing destination. Native torrents upload while active and stop seeding on completion or pause. Resume rechecks existing pieces. Metalink descriptors are limited to 8 MiB / 1024 files, reject unsafe paths and XML DTDs, and require a supported whole-file checksum for each file. A bad mirror or checksum is tried against the next mirror; no unverified file is published.

Video hosts automatically recognized include YouTube, Vimeo, Twitch, TikTok, and Instagram, including their subdomains; `--ytdlp` selects the extractor for other supported sites. Site changes, login requirements, DRM, and availability can prevent an extraction. Browser cookies are not exported automatically. Pause stops the media process tree and retains resumable files.

The `torrent-native` feature is enabled by default and included in the installer. To omit it, run `.\run.ps1 build -CargoArguments @('--no-default-features')`. `run.ps1` forwards `-CargoArguments` to Cargo for targeted tests and feature checks.

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

Capture is enabled by default: start a normal browser download and it is sent to Strata automatically, with no **Download with Strata** action required. Accepted browser downloads open or restore the app, including when it is hidden in the tray. Downloads awaiting browser scanning are retried when the browser reports them safe. The context-menu command and popup URL field remain optional shortcuts. Checking connection status alone does not open the app.

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

Authenticated browser-cookie import, scheduling, speed limits, and dynamic redistribution of a slow HTTP connection's remaining range are outside this version's scope. Direct-file names come from the browser when provided, or from the URL path; Content-Disposition names are not yet read directly by the app. Cancellation stops the transfer and retains its partial files; remove the matching hidden parts folder manually if you no longer need it.

## Checks

The local HTTP test server verifies simultaneous range requests and byte-for-byte assembly, ordinary full-file fallback, empty files, pause/resume across manager restart, remote file changes, malformed range rejection, collision protection, filename sanitation, and range partitioning. It never downloads test files from the public internet.

Loopback transport tests verify WebDAV authentication, authenticated SOCKS5 with remote DNS, HTTP proxy routing, FTP anonymous/password authentication and REST resume, SFTP password/key authentication and resume, and Metalink mirror recovery after checksum failure. A real local BitTorrent seed verifies `.torrent` and magnet transfers and rechecking existing partial data. Host-key tests reject unknown, changed, and revoked keys. Credential tests check protected persistence and diagnostic redaction. The optional `ytdlp_download_pause_and_resume_are_byte_exact` test uses the pinned helper against a local media fixture; run it with `.\run.ps1 test -CargoArguments @('--test','downloads','ytdlp_download_pause_and_resume_are_byte_exact','--','--ignored','--exact')`.

`node --test extension/background.test.js` checks capture ordering, failure recovery, capture-off behavior, private/unsafe URL exclusions, cancellation failures, and worker restart recovery. Rust also checks native-message framing, named-pipe handoff, filename/referrer forwarding, and rejecting a transfer when history cannot be persisted.

Tray checks cover mouse and keyboard callback routing. The ignored `native_tray_hides_restores_and_cleans_up` test requires interactive Windows Explorer; it creates its own window and verifies native tray creation, restore, hide without destroying the window, and cleanup. It was also run successfully on Windows.

End-to-end verification used isolated Chromium and Brave profiles: a real browser download was handed through the native host to the app, the browser's copy was canceled after acceptance, and the app's 4 MiB file matched every source byte. With capture off, the next file completed in the browser without creating an app transfer. The popup rendered its live connected state without script errors. Brave was checked against the optimized release app.

`scripts/test-installer.ps1` exercises the real setup EXE in a directory with spaces: running-app protection, install, repeat-install upgrade, runtime and extension payloads, Start menu shortcut, native host registrations, and uninstall. It starts the installed CLI and each media helper with a restricted PATH, checks that unrelated data remains, and verifies that a bridge registration belonging to a different installation survives. Use a Windows account without an existing installed Strata or Strata Start menu shortcut; quit any running copy first. The test snapshots and restores existing native host registrations and leaves diagnostic logs under `.tools/installer-test`.

The editable text input is adapted from GPUI's Apache-2.0 example; see `NOTICE` and `LICENSE-APACHE`.
