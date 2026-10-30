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

The protocol scope also includes HTTP(S) / SOCKS5 proxying, FTP REST resume, SFTP password/key authentication with known-host verification, WebDAV Basic auth, native v1/hybrid BitTorrent and magnets, checksum-verified Metalink v3/v4 mirrors, and video extraction through yt-dlp. Protocols are detected in the GUI and can be forced by the companion CLI. URL and proxy passwords and key passphrases are protected with Windows DPAPI. Multi-file outputs are staged and published without replacing user data. The installer bundles the CLI and pinned media helpers.

## Brand Commitments
The download table fills the window without a left sidebar, workspace label, or breadcrumbs. Filters and Settings live in the top toolbar. Row menus expose contextual actions; Delete from device permanently removes owned output and partial data after writes stop. A draggable bottom inspector remembers its height and uses one byte-weighted segmented progress bar. Automatic browser captures open or restore 101 Downloader after acceptance.

The user approved a Windows-first, calm dark interface with pause/resume, history, and visible parallel connections. The user selected the name 101 Downloader and supplied an orange/black/gray palette reference. Existing Strata history and browser identity must survive upgrades.

## Evidence on Hand
The workspace was empty. No existing assets or download history. Empty states must show honest zeros; preview data must be clearly identified.

New download is a centered modal with keyboard focus containment. The queue includes ETA and omits transfer summary cards. Its empty detail prompt is centered. Native inputs share blinking carets. The browser companion suppresses download UI while capturing, waits for filename determination, and the app recovers server-provided names before final publication.
