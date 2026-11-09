# 101 Downloader

A native Windows download manager built with Rust and GPUI. Downloads run in a persistent queue with parallel HTTP connections, pause/resume, browser capture, torrent file selection, and per-download or global speed limits.

## Installation

Windows installers and SHA-256 checksums are available from [GitHub Releases](https://github.com/tarus-eugene/Strata/releases/latest). The current installer is `101-Downloader-Setup-0.3.6-x64.exe`.

The installer supports Windows 10 version 2004 or later and Windows 11 on x64-compatible systems. A working GPU driver is required. Installation is per Windows account and does not require administrator access. The default location is `%LOCALAPPDATA%\Programs\101 Downloader`.

The package includes the desktop app, command-line client, native browser bridge, Chromium companion extension, Visual C++ runtime DLLs, and pinned yt-dlp, FFmpeg, and Deno helpers. The installer is unsigned.

The app must be fully closed through **Quit 101 Downloader** in the tray menu before an upgrade or uninstall. Closing the main window only hides it to the tray. Upgrades retain history, settings, partial transfers, and downloaded files. Uninstall also retains download data.

## Download management

| Control | Behavior |
| --- | --- |
| **New download** / **Ctrl+N** | Accepts a URL, magnet link, or local torrent/Metalink descriptor. A valid clipboard source prefills the form. |
| **Search** / **Ctrl+F** | Filters the download table. Filter, Settings, and New download occupy the right side of the toolbar; selection actions occupy the left. |
| Row selection | Checkboxes, **Ctrl+click**, **Shift+click**, and **Ctrl+Shift+click** support individual, range, and additive selection. The header checkbox selects visible rows; **Ctrl+A** selects them when the table has focus. |
| Row context menu | Provides pause/resume, rename, priority, speed limit, open file/folder, copy link, cancel, and permanent deletion. Batch-capable actions apply to the current selection. |
| **Rename** | Prefills the current name and selects the filename stem without its last extension. Folder names are selected in full. Existing paths are never replaced. |
| **Set priority** | High, Normal, and Low determine which queued transfer starts next. Equal priorities retain creation order; active transfers are not preempted. |
| Double-click | Opens a completed download's file or output folder. |
| **Cancel** | Stops a transfer while retaining partial data. |
| **Delete from device** | Waits for writes to stop, deletes owned output and partial data, and removes the entry after successful cleanup. Failures leave the entry available for retry. |

Up to three downloads run simultaneously. In Settings, choose Auto or a limit of 1, 4, 8, or 16 connections. Auto uses the HTTP file size and server range support to choose a count. Auto is the default for new installations, and saved settings keep their selected count.

Hover over a clipped table row to see the full filename. Left-drag a completed row to move its file or folder, or middle-drag it to copy into File Explorer or another app that supports file drops. For moves on the same drive, the download entry follows the item to its new location. Moves between drives may still show **Missing**.

Downloads play a Windows chime when they finish. Use **Settings → Completion chime** to turn it off.

Use **On download complete** in an unfinished download's context menu to choose a one-time action for this session. Choosing an option applies it and closes the dialog. **Do nothing** clears the action. The PC can shut down, sleep, or restart after that download finishes. **Open** launches a Windows executable or opens the download folder.

The bottom details panel shows the selected download's full filename, destination, source, completion time, priority, limit, and segmented progress. Its divider supports dragging and **Up/Down** keyboard resizing, and the height is remembered. The footer chevron collapses or restores the panel. Tables, Settings, and overflowing dialogs have draggable scrollbars. Smooth wheel scrolling respects Windows animation preferences.

The **Created** column and **Completed at** details use Windows date and local-time preferences, including the system's 12/24-hour setting. Older completed entries without a saved completion timestamp show a dash. Speed and ETA use a six-second rolling window, time-based smoothing, and at most one speed update per second. Estimates warm up for two seconds; a three-second stall clears stale speed and ETA. Unknown-size and stopped transfers have no ETA. Search highlights matching filename text and shows matching websites beneath filenames.

New downloads save SHA-256 fingerprints for every output file. Direct transfers verify the assembled file against the downloaded parts before publishing; Metalink checks publisher hashes and torrents verify pieces. **Verify file integrity** in row context menus and selection actions checks saved fingerprints again, including same-size corruption. Fingerprints for sources without publisher checksums detect later local changes; they do not authenticate the publisher's original. For older downloads, the action checks recorded file sizes and saves an initial checksum. The details panel shows **Initial checksum recorded**; this first baseline cannot rule out earlier same-size corruption.

Completed downloads become **Missing** when the file, output folder, or a recorded file inside it disappears. Checks run in the background and at startup. Returning the output to its saved location restores its completed status; **Verify file integrity** can then check its bytes.

### Speed limits

**Speed limit** in a row context menu applies a limit to each selected download, shared across its connections. **Settings → Global speed limit** applies a shared cap. Values are whole KiB/s; **0** means unlimited. Nonzero global and torrent limits start at 16 KiB/s.

Under a global cap, FTP, video, torrent, and Metalink jobs run exclusively so their separate rate controls cannot exceed the shared limit. HTTP and SFTP jobs can share the cap concurrently. Changing a limit pauses affected active transfers briefly, applies the setting, and resumes them. Priorities, limits, and renamed destinations persist across restarts.

### Torrents

Torrents load metadata before presenting an expandable file tree. Folder checkboxes select descendants and display partial-selection states; individual file checkboxes and **Select all** control the selection. The dialog shows the selected file count and total size. Payload downloading starts only after confirmation.

Canceling the picker leaves the job paused; Resume reopens it. Confirmed choices persist across restarts. Only selected files are published, and empty unselected folders are removed. The same selection flow applies to browser and CLI handoffs. Native torrents upload while active and stop seeding on completion or pause. Resume rechecks existing pieces.

The torrent context menu's **Options** action reopens file selection after a transfer stops or completes. Previous selections remain selected. Adding files stages existing payloads privately for piece verification and reuse, downloads the additional selection, and publishes the expanded folder at the same destination.

### Tray and updates

Closing the main window hides the app to the system tray while transfers continue. The tray icon, its **Open 101 Downloader** command, and subsequent app launches restore the existing window. **Quit 101 Downloader** pauses active transfers, saves history, and exits; transfers reopen paused.

The app checks this repository's public GitHub Releases at startup. **Settings → App updates → Check for updates** performs a manual check. **Download update** adds a newer stable Windows installer to the normal queue. Downloading and installation both require explicit actions.

## Supported protocols

| Protocol | Source / override | Support |
| --- | --- | --- |
| HTTP / HTTPS | Automatic | Parallel ranges, validated resume, original file bytes; HTTP, HTTPS, SOCKS5, and SOCKS5h proxies |
| FTP | `ftp://` / `--ftp` | Anonymous or authenticated binary transfers with REST resume |
| SFTP | `sftp://` / `--sftp` | Password or OpenSSH key authentication, strict known-host verification, seek resume |
| WebDAV | `webdav://`, `webdavs://` / `--webdav` | HTTP(S) GET with Basic authentication and normal range behavior |
| BitTorrent | Magnet, local `.torrent`, or torrent URL | Native v1/hybrid torrents, trackers, DHT, file selection, and piece verification |
| Metalink | `.meta4`, `.metalink` / `--metalink` | v3/v4 descriptors, mirror failover, multiple files, SHA-256/SHA-512/MD5 verification |
| Video sites | Recognized host / `--ytdlp` | yt-dlp extraction with FFmpeg merging and Deno JavaScript support |

Torrent, Metalink, and video transfers stage files privately and publish a download folder without replacing an existing destination. Metalink descriptors are limited to 8 MiB and 1024 files, reject unsafe paths and XML DTDs, and require a supported whole-file checksum for each file. Failed mirrors or checksums trigger the next mirror.

Recognized video hosts include YouTube, Vimeo, Twitch, TikTok, and Instagram; `--ytdlp` selects the extractor for other supported sites. Site availability, login requirements, DRM, and upstream changes can prevent extraction. Browser cookies are not imported automatically. Pausing stops the media process tree and retains resumable files. Scheduling is not currently implemented.

## Command line

`101Downloader-cli.exe` queues transfers in the desktop app and starts it when necessary. The desktop executable accepts the same transfer flags; the console client reports command-line errors and queue IDs.

```powershell
.\101Downloader-cli.exe https://example.com/large.zip --connections 8
.\101Downloader-cli.exe https://example.com/large.zip --proxy socks5h://localhost:1080
.\101Downloader-cli.exe --ftp ftp.example.com/pub/file.zip
.\101Downloader-cli.exe --sftp alice@example.com/archive.zip --identity C:\Users\alice\.ssh\id_ed25519
.\101Downloader-cli.exe --webdav https://alice:password@example.com/archive.zip
.\101Downloader-cli.exe 'magnet:?xt=urn:btih:INFO_HASH'
.\101Downloader-cli.exe C:\Downloads\release.torrent
.\101Downloader-cli.exe --metalink https://example.com/release.meta4
.\101Downloader-cli.exe --ytdlp https://www.youtube.com/watch?v=VIDEO_ID
```

Explicit transfer options include `--user`, `--password-stdin`, `--identity`, `--key-passphrase-stdin`, `--known-hosts`, `--directory`, and `--connections`. Standard-input password flags avoid including secrets in command arguments or shell history:

```powershell
$credential = Get-Credential -UserName alice
$credential.GetNetworkCredential().Password | .\101Downloader-cli.exe sftp://alice@example.com/archive.zip --password-stdin
```

URL and proxy passwords and key passphrases are protected with Windows DPAPI before history is saved. They are bound to the current Windows account and require re-entry after migration to another account or machine. Displayed source links omit user information; URL query parameters remain part of the saved link.

SFTP defaults to `%USERPROFILE%\.ssh\known_hosts` and rejects unknown, changed, or revoked keys. Verified OpenSSH host entries or `--known-hosts` provide host-key trust. CA-only entries do not authorize an unpinned key. Encrypted private keys use `--key-passphrase-stdin`. Proxy settings apply to HTTP(S), WebDAV, Metalink HTTP mirrors, and video transfers; FTP, SFTP, and torrent peers use direct connections.

## Chromium browser companion

The Manifest V3 companion supports compatible Windows Chromium browsers, including Chrome, Edge, Brave, Vivaldi, Opera/Opera GX, and Chromium. Managed browser policies and fork-specific restrictions can prevent unpacked extensions or native messaging.

The installer registers the native bridge for the current Windows account. **Settings → Browser companion → Connect browsers** and `install-browser.ps1` provide registration for source builds.

1. The browser's extensions page (`chrome://extensions`, `edge://extensions`, `brave://extensions`, `vivaldi://extensions`, or `opera://extensions`) provides the **Developer mode** switch.
2. **Load unpacked** accepts the installed or checked-out `extension` folder. **Settings → Extension folder** opens its location.
3. The **101 Downloader — Download companion** popup shows connection status and the capture switch.

Capture is enabled by default. Eligible browser downloads are paused during handoff and canceled only after the app confirms that the transfer was saved to its queue. A failed handoff resumes the browser's copy. Accepted downloads restore the desktop app. Browser safety scanning is respected, and pending handoffs are recovered after extension-worker restarts.

The extension sends the URL, browser filename, and referrer. It has no page injection, browsing-history access, or cookie/password import. Browser-generated `blob:` files, private downloads, already-paused downloads, and flagged files remain in the browser. Login-only and POST-generated downloads may require capture to be disabled. Queue acceptance confirms handoff; subsequent transfer errors appear in the desktop app.

The `downloads.ui` permission suppresses the browser download interface while capture is enabled. Capture-off and browser fallback restore it. This control is profile-wide and may be unavailable in some Chromium forks. Other permissions are `downloads`, `nativeMessaging`, `storage`, and `contextMenus`.

The desktop app and bridge executable must remain together; moving them requires bridge registration to be refreshed. Registration commands for source builds are:

```powershell
.\install-browser.ps1
.\install-browser.ps1 -AppDirectory .\target\release
.\install-browser.ps1 -Uninstall
```

Unregistration retains history and downloads. Installer uninstall preserves a registration that has since been assigned to another installation.

The bridge uses Chromium's [native messaging protocol](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging) and a local named pipe restricted to the current Windows account and SYSTEM. The extension's committed public key fixes its ID at `eaccbjfepcpidchboonodedpepgboolc`. Messages are length-bounded, limited to fixed actions, and accepted only from that extension origin. Registration uses the Chrome, Chromium, and Edge per-user native messaging locations; compatible forks may use those lookups.

## Building from source

The build requires stable Rust, Visual Studio C++ Build Tools, and a Windows SDK. `run.ps1` discovers the C++ environment and uses the project-local Rust installation when available. Dependency versions are recorded in `Cargo.lock`.

```powershell
.\run.ps1
.\run.ps1 build
.\run.ps1 build -Release
```

Source builds produce `strata.exe`, `strata-cli.exe`, and `strata-bridge.exe` under `target\debug` or `target\release`. Installer names are `101Downloader.exe`, `101Downloader-cli.exe`, and `101Downloader-Bridge.exe`. The executables run independently of Cargo, without a web server or browser.

The `torrent-native` feature is enabled by default. `run.ps1` forwards `-CargoArguments` for feature selection and targeted checks:

```powershell
.\run.ps1 build -CargoArguments @('--no-default-features')
```

### Installer packaging

Packaging requires [Inno Setup](https://jrsoftware.org/isdl.php) 6.3 or later and the Visual Studio C++ build tools:

```powershell
.\scripts\package-installer.ps1
```

The script builds release executables and writes a versioned setup EXE, SHA-256 file, and `101-downloader-browser-extension.zip` to `dist`. `-SkipBuild` packages an existing release build; `-CompilerPath` selects `ISCC.exe`; `-RuntimeDirectory` selects an x64 Visual C++ redistributable folder. Staging is stored under the ignored `.tools` directory. Installed systems do not need Rust.

Media helpers come from their official release repositories and are checked against sizes and SHA-256 hashes in `resources/media-tools.json`. Their licenses and source/build links accompany the installed tools. `scripts/fetch-media-tools.ps1` populates the development cache; helpers can also reside beside the app in `tools` or on PATH. Initial packaging requires network access.

### Transfer implementation

HTTP downloads probe with `Range: bytes=0-0` and request identity encoding. Parallel transfers require a valid `206 Content-Range`, a known size, and a strong ETag or Last-Modified validator. Non-overlapping ranges enter a shared queue with a target size of 4 MiB and at least the configured worker count. Workers reuse pooled connections and maintain separate part files.

Every range response is checked for size, encoding, range bounds, and validator consistency. Broken connections are retried up to three times from the actual saved length. A transfer interrupted by a temporary network or server error gets up to three more attempts, with waits of one, two, and four seconds. You can pause or cancel during the wait. Idle workers can split a slow range's remaining bytes after its writer stops and flushes. Resume probes compare size and validator before reusing parts. Completed parts are assembled in order and published without replacing an existing path.

Servers without range support, validators, or a known size use a single full-file transfer that restarts on resume. Parallel connections cannot increase the underlying connection's bandwidth.

### Storage and rendering

New installations store history and settings in `%LOCALAPPDATA%\101 Downloader\history.json`. Existing `%LOCALAPPDATA%\Strata\history.json` remains supported. Partial data is stored in `.strata-<id>.parts` under the download destination. History writes are atomic, and a process lock prevents concurrent writers.

`DOWNLOADER101_DATA_DIR` selects a portable or isolated history/settings directory; `STRATA_DATA_DIR` remains a legacy alias. Existing download destinations are unaffected.

The native interface uses Segoe UI, opaque surfaces, per-monitor DPI awareness, and physical-pixel-aligned text baselines. GPUI 0.2.2 uses DirectWrite grayscale antialiasing. The shared logo source is `assets/brand/mark.png`; `scripts/generate-icons.ps1` generates platform icon variants.

### Validation

```powershell
.\run.ps1 test
.\run.ps1 clippy
node --test extension/background.test.js
```

Automated coverage includes byte-exact parallel assembly, range validation, connection reuse, slow-tail splitting, pause/resume, filename recovery, collision protection, deletion, rate limits, speed/ETA sampling, proxy authentication, FTP/SFTP/WebDAV, Metalink verification, local torrent/magnet transfers, torrent file selection, credential protection, and browser handoff recovery. Download fixtures use local servers.

Optional tests require additional resources: `ytdlp_download_pause_and_resume_are_byte_exact` uses pinned media helpers, and `native_tray_hides_restores_and_cleans_up` requires interactive Windows Explorer. `scripts/test-installer.ps1` exercises installation, upgrade, uninstall, payloads, shortcuts, native registrations, and running-app protection. It requires an isolated Windows account without an existing installation or either product's Start menu shortcuts and writes diagnostics under `.tools/installer-test`.

Repository documentation is maintained in this README. Generated planning and release-note Markdown is excluded from version control; published release notes are available on GitHub Releases.

## Licensing

The project license is in `LICENSE`. The text input adapts GPUI's Apache-2.0 example; attribution is in `NOTICE` and `LICENSE-APACHE`. Bundled media helpers retain their upstream licenses and accompanying notices.
