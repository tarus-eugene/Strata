# Strata

<!-- impeccable:product-schema 1 -->

## Platform
Windows desktop, native Rust and GPUI.

## Stack
Rust and GPUI, explicitly requested. HTTP byte-range downloads run in background workers.

## Product Purpose
An IDM-like download manager with a beautiful, calm dark interface. Users add URLs, manage transfers, pause and resume, and inspect simultaneous connections and download history.

## Capabilities and Constraints
Split supported HTTP downloads into concurrent parts. Safely fall back when ranges are unavailable. Preserve partial downloads, avoid overwriting user files, and report failures clearly. Investigate GPUI text blur and apply supported mitigations.

## Brand Commitments
The user approved a Windows-first, calm dark interface with pause/resume, history, and visible parallel connections. Strata is a working product name selected for this build.

## Evidence on Hand
The workspace was empty. No existing assets or download history. Empty states must show honest zeros; preview data must be clearly identified.
