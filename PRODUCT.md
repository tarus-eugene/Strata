# 101 Downloader

<!-- impeccable:product-schema 1 -->

## Platform
Windows desktop, native Rust and GPUI.

## Stack
Rust and GPUI, explicitly requested. HTTP byte-range downloads run in background workers.

## Product Purpose
An IDM-like download manager with a beautiful, calm dark interface. Users add URLs, manage transfers, pause and resume, and inspect simultaneous connections and download history.

## Capabilities and Constraints
Split supported HTTP downloads into concurrent parts. Safely fall back when ranges are unavailable. Preserve partial downloads, avoid overwriting user files, and report failures clearly. Investigate GPUI text blur and apply supported mitigations.

The user additionally requested a custom in-app title bar, a logo, and a Chrome extension that captures file downloads and funnels them to the desktop app. The implementation uses a per-user local native-messaging bridge and preserves Chrome downloads when handoff fails.

The browser companion now targets compatible Windows Chromium browsers, including Chrome, Edge, Brave, Vivaldi, Opera / Opera GX, and Chromium. A per-user Windows installer bundles the release app, bridge, extension, runtime DLLs, and shortcuts. Loading the extension remains an explicit action in each browser profile. Upgrades require quitting 101 Downloader through its tray menu; uninstall preserves files, partial transfers, settings, and history.

The user requested close-to-tray behavior: closing the main window keeps the app and transfers running in the background. The tray icon restores it and provides an explicit Quit action.

The app checks public GitHub Releases at startup and offers to download a newer stable Windows installer through the existing queue. Settings includes a manual check and retry. Installation remains explicit and requires quitting the app through its tray menu.

The protocol scope also includes HTTP(S) / SOCKS5 proxying, FTP REST resume, SFTP password/key authentication with known-host verification, WebDAV Basic auth, native v1/hybrid BitTorrent and magnets, checksum-verified Metalink v3/v4 mirrors, and video extraction through yt-dlp. Protocols are detected in the GUI and can be forced by the companion CLI. URL and proxy passwords and key passphrases are protected with Windows DPAPI. Multi-file outputs are staged and published without replacing user data. The installer bundles the CLI and pinned media helpers.

Row checkboxes and Ctrl/Shift/Ctrl+Shift selection support individual toggles, ranges in the current displayed order, and additive ranges. The header checkbox selects or clears visible rows; Ctrl+A selects visible rows when the table has focus. Right-click and three-dot menus provide rename, speed limits, High/Normal/Low queue priority, and batch pause/resume/cancel/delete. Rename opens the current name with the filename stem selected, excluding its extension; folder outputs select the whole name.

Per-download speed limits apply to each selected download across its connections. Settings uses the same modal for a shared global cap. Limits use whole KiB/s, with 0 for unlimited and a 16 KiB/s minimum for nonzero global and torrent limits. Under a global cap, FTP, video, torrent, and Metalink jobs run exclusively to preserve the shared limit; HTTP and SFTP can share it concurrently.

## Brand Commitments
The download table fills the window without a left sidebar, workspace label, or breadcrumbs. Filters and Settings live in the top toolbar. Row menus expose contextual actions; Delete from device permanently removes owned output and partial data after writes stop. A compact bottom inspector uses one byte-weighted segmented progress bar, has no internal scrolling or action buttons, and retains drag/keyboard resizing with a remembered height and 180 px minimum. The permanent window-bottom footer collapses it with a down chevron and restores it with an up chevron. Accepted downloads bring the app to the foreground. The window title is simply 101 Downloader.

Created dates in the table and Completed at date/time in the inspector follow Windows locale and local time, including the system's 12/24-hour preference. Older completed entries without a recorded completion timestamp show a dash.

The user approved a Windows-first, calm dark interface with pause/resume, history, and visible parallel connections. The user selected the name 101 Downloader and supplied an orange/black/gray palette reference. Existing Strata history and browser identity must survive upgrades.

## Evidence on Hand
The workspace was empty. No existing assets or download history. Empty states must show honest zeros; preview data must be clearly identified.

New download is a centered modal with keyboard focus containment. The queue includes ETA and omits transfer summary cards. Its empty detail prompt is centered. Native inputs share blinking carets. The browser companion suppresses download UI while capturing, waits for filename determination, and the app recovers server-provided names before final publication.

Scrollable tables, Settings, and overflowing dialogs share visible proportional scrollbars with drag, track-click, and keyboard control. Wheel motion eases at animation-frame cadence, reverses immediately, and respects Windows animation preferences; precise pixel input remains direct. Settings omits its introductory tagline and avoids periodic progress-driven repaints while open.
