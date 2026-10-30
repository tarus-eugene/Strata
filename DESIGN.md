# Strata interface

Native Windows utility in Operate mode. The user approved a calm, dark IDM alternative. The native implementation is the visual authority.

The frame uses neutral near-black, a full-width download table, and a transfer inspector docked beneath the table. The work is always in the middle: add a URL, read progress, pause or resume. The signature is one segmented progress track: each segment occupies its real byte range and fills as that part arrives.

Surface #0c0d0f, raised #151719, controls #242629, rules #252729. Text #f0f1f2, secondary #b4b6b8, metadata #92969a. Sky blue #82b9e7 marks transferred bytes; amber means paused, soft red means failure. White primary controls. No gradients, blur, shadows, fabricated statistics, or sample transfers in the normal app.

Segoe UI uses the installed Windows face deliberately for native text rasterization; title 23–25 px, body 14 px, metadata 12 px, summary 24 px. Readable integer font sizes, opaque text backgrounds, explicit line heights. GPUI's windows-manifest feature embeds PerMonitorV2 DPI awareness. A 44 px custom title bar replaces the operating-system caption, with native draggable hit areas, snap-compatible window controls, and explicit maximize/restore behavior. The left edge holds the Strata mark and wordmark; close turns red on hover.

Minimum window 1020 × 700; default 1380 × 880. No left navigation, workspace label, or breadcrumbs. Filter is at the top left; Settings and New download are at the top right. The filter menu contains All downloads, In progress, Completed, and Stopped. Below 1200 px the speed column is hidden. Table columns align across the header and 48 px rows: file name, size, status, progress, speed, and a three-dot action menu. Thin horizontal rules separate rows; rows have no card gaps. Contextual actions include pause/resume, open, open folder, copy download link, cancel, and delete. Delete removes history and stops work while retaining files.

The bottom inspector has an 8 px draggable divider, keyboard Up/Down resizing, a remembered height, and bounds that preserve table space. Its single full-width segmented progress bar represents the actual relative byte length and received portion of each download part. Without selection, it displays “Click on a download to view more info.” Content, settings and details scroll independently. Progress tracks keep a fixed physical layout height. Add-download is an inline panel with real editing, selection, clipboard and IME support. Ctrl+N adds, Ctrl+F searches, Enter starts, Esc dismisses a menu or closes the form, and Tab moves focus. Popup menus support Up/Down, Enter, Escape, and outside-click dismissal.

Empty states retain real zero counters. Filtered-empty states explain how to recover. History and partial files persist automatically. The UI reports background persistence errors visibly.

Both the custom close button and native close requests hide the existing window to the system tray. Transfers and browser handoffs continue. The shared Strata icon restores the window on selection and exposes a native Open/Quit menu; only Quit pauses transfers and exits. Starting Strata again or choosing Open app from the extension restores the same window.

The shared logo consists of three filled downward ribbons in #82b9e7. Its editable source is assets/brand/mark.svg. Platform ICO/PNG exports derive from that SVG. The browser popup inherits the neutral dark surfaces, the same mark, one capture switch, one link input, and an honest native-connection status. Setup and failures explain how to recover; browser captures are never represented as successful before the app acknowledges them. Companion copy refers to the user's browser, with setup instructions for Chromium variants rather than assuming Chrome.

Panels, navigation items, inputs, buttons, progress tracks, status markers, and the browser companion's capture switch use square corners with no border radius, as requested by the user.

Browser capture is automatic by default and restores Strata after queue acceptance. Browser safety scanning is respected; transfers are retried when scanning reports them safe. The browser's paused copy is canceled only after the app accepts it.
