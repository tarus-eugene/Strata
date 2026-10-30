# Strata interface

Native Windows utility in Operate mode. The user approved a calm, dark IDM alternative. The native implementation is the visual authority.

The frame uses neutral near-black, a flat sidebar, an aligned queue, and a persistent transfer inspector. The work is always in the middle: add a URL, read progress, pause or resume. The signature is a separate progress lane for every real HTTP connection.

Surface #0c0d0f, raised #151719, controls #242629, rules #252729. Text #f0f1f2, secondary #b4b6b8, metadata #92969a. Sky blue #82b9e7 marks transferred bytes; amber means paused, soft red means failure. White primary controls. No gradients, blur, shadows, fabricated statistics, or sample transfers in the normal app.

Segoe UI uses the installed Windows face deliberately for native text rasterization; title 23–25 px, body 14 px, metadata 12 px, summary 24 px. Readable integer font sizes, opaque text backgrounds, explicit line heights. GPUI's windows-manifest feature embeds PerMonitorV2 DPI awareness. A 44 px custom title bar replaces the operating-system caption, with native draggable hit areas, snap-compatible window controls, and explicit maximize/restore behavior. The left edge holds the Strata mark and wordmark; close turns red on hover.

Minimum window 1020 × 700; default 1380 × 880. Navigation is 212 px and the inspector 304 px; below 1200 px these become 188 px and 272 px, with the queue's speed column hidden to preserve file-name space. Content, settings and details scroll independently. Progress tracks keep a fixed physical layout height. Add-download is an inline panel with real editing, selection, clipboard and IME support. Ctrl+N adds, Ctrl+F searches, Enter starts, Esc closes, Tab moves focus.

Empty states retain real zero counters. Filtered-empty states explain how to recover. History and partial files persist automatically. The UI reports background persistence errors visibly.

Both the custom close button and native close requests hide the existing window to the system tray. Transfers and browser handoffs continue. The shared Strata icon restores the window on selection and exposes a native Open/Quit menu; only Quit pauses transfers and exits. Starting Strata again or choosing Open app from the extension restores the same window.

The shared logo consists of three filled downward ribbons in #82b9e7. Its editable source is assets/brand/mark.svg. Platform ICO/PNG exports derive from that SVG. The Chrome popup inherits the neutral dark surfaces, the same mark, one capture switch, one link input, and an honest native-connection status. Setup and failures explain how to recover; browser captures are never represented as successful before the app acknowledges them.
