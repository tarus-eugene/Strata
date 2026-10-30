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
- Pause/resume individual downloads, pause all, or cancel a transfer. History survives closing the app. Running downloads reopen paused; they never silently restart.
- Choose the download folder and 1, 4, 8, or 16 connections in Settings. Up to three downloads run simultaneously.
- Open a finished file or its folder from the inspector. Strata never automatically opens downloaded files.

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

This first version handles direct file links. Browser interception, authenticated cookie import, video extraction, scheduling, speed limits, and dynamic redistribution of a slow connection's remaining range are outside its scope. Names come from the URL path; Content-Disposition names are not yet read. Cancellation stops the transfer and retains its partial files; remove the matching hidden parts folder manually if you no longer need it.

## Checks

The local HTTP test server verifies simultaneous range requests and byte-for-byte assembly, ordinary full-file fallback, empty files, pause/resume across manager restart, remote file changes, malformed range rejection, collision protection, filename sanitation, and range partitioning. It never downloads test files from the public internet.

The editable text input is adapted from GPUI's Apache-2.0 example; see `NOTICE` and `LICENSE-APACHE`.
