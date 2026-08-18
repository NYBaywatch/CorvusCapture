<!-- GSD:project-start source:PROJECT.md -->

## Project

**Corvus Capture**

Corvus Capture is a lightweight Windows 11 screenshot utility, written in Rust, that lives in the system tray and responds to global hotkeys. It captures the full screen of the monitor under the cursor (`F9`) or a user-adjusted region of a frozen screen (`Shift+F9`), and saves files automatically using a configurable base name with zero-padded auto-numbering — no dialogs, no prompts, files just appear. Built for public release as a single portable exe.

**Core Value:** Hotkey-to-file capture with zero friction: press a key, a correctly-named numbered file appears in the configured folder in under 100 ms — while the app idles at under 10 MB RAM and 0% CPU.

### Constraints

- **Platform**: Windows 11 x64 only — target audience and Win32-specific implementation
- **Tech stack**: Rust with the `windows` crate; raw Win32 for the Settings UI — smallest footprint, user preference
- **Performance**: < 10 MB idle RAM, 0% idle CPU, < 5 MB binary, < 100 ms hotkey→file (fullscreen), < 50 ms overlay appearance — core value; drives crate and architecture choices
- **Distribution**: single portable .exe, no installer — zero-friction adoption for MVP

<!-- GSD:project-end -->

<!-- GSD:stack-start source:research/STACK.md -->

## Technology Stack

## Verdict on the Proposed Stack

## Recommended Stack

### Core Technologies

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| Rust (stable, MSVC toolchain) | 1.8x stable | Language/toolchain | Zero-runtime, direct Win32 FFI, tiny binaries with size flags. Use `x86_64-pc-windows-msvc`. |
| `windows` | 0.62.2 | Win32 API bindings (GDI, DPI, window classes, mutex, shell) | Microsoft-maintained, feature-gated so only used API families link in. Verified current: 0.62.2 (Oct 2025). |
| `tray-icon` | 0.24.2 | System tray icon + context menu (bundles `muda` 0.19.x) | Tauri-maintained, explicitly works with a **raw win32 message pump** — winit is NOT required, only "a win32 event loop on the same thread" (verified in docs). Actively released (July 2026). |
| `global-hotkey` | 0.8.0 | RegisterHotKey wrapper for F9/Shift+F9/Alt+F9 | Same Tauri family, same raw-message-pump compatibility, event receiver model matches tray-icon. 0.8.0 (May 2026). |
| `image` | 0.25.10 | PNG/JPG/BMP/WebP(lossless) encoding | Pure Rust, no C toolchain, per-codec feature gates keep binary small. Enable only `png`, `jpeg`, `bmp`, `webp` features (`default-features = false`). |
| `serde` + `serde_json` | 1.0.229 / 1.0.151 | `config.json` read/write | Standard, negligible weight with `derive` only. |

### Supporting Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `dirs` | 6.0.0 | Resolve `%APPDATA%` | Optional — `std::env::var("APPDATA")` is sufficient on Windows-only; use `dirs` only if you want the safety of SHGetKnownFolderPath semantics. Recommendation: skip it, use the env var. |
| `webp` (libwebp bindings) | 0.3.1 | **Lossy** WebP with quality parameter | Only if the quality slider must apply to WebP. Costs: C build via `libwebp-sys` (needs cc/clang), ~300–500 KB binary growth. Recommendation: defer; ship lossless WebP via `image` in MVP. |
| `zopfli` | 0.8.3 | Max-compression PNG | Do NOT use for the hot path (far too slow). Listed only so it isn't accidentally adopted. |

### Development Tools

| Tool | Purpose | Notes |
|------|---------|-------|
| `cargo` + release profile below | Size-optimized builds | See flags section — this is what gets you under 5 MB. |
| `winres` or `embed-resource` | Embed .ico + version info + PMv2 DPI manifest | The **application manifest** (Per-Monitor V2 DPI awareness) must be embedded at link time — do this via a `.rc`/manifest in `build.rs`, not `SetProcessDpiAwarenessContext` alone (manifest is the Microsoft-recommended path and applies before any window is created). |
| `cargo-bloat` | Verify binary size budget | Run per-milestone to catch dependency creep. |
| Task Manager / `procexp` | Verify <10 MB idle RAM | Measure Private Working Set after tray idle + one capture. |

## Release Profile (binary size)

## Installation

## Alternatives Considered

| Recommended | Alternative | When to Use Alternative |
|-------------|-------------|-------------------------|
| Raw Win32 message pump | `winit` 0.30 | If you needed cross-platform or complex windowing. Costs ~0.5–1 MB binary, extra idle RAM, and fights you on tray/hidden-window patterns. Not justified for a Windows-only tray app. |
| Hand-rolled GDI BitBlt | `xcap` 0.9.8 / `windows-capture` 2.0.1 | `xcap` is cross-platform (drags in extra deps); `windows-capture` uses Windows.Graphics.Capture (WinRT, D3D11 — heavier, shows capture border on some builds, more idle machinery). BitBlt via `windows` is ~40 lines and you control DPI/monitor handling exactly. Revisit `windows-capture`/DXGI post-MVP for HDR. |
| Raw Win32 Settings window | `egui`/`iced`/`slint`/Tauri | Any of these adds 2–15 MB binary and continuous or spiky RAM. A one-page settings dialog in raw Win32 (or even a `DialogBox` from an embedded .rc) is small and idles at zero. |
| `image` (pure Rust encoders) | `mozjpeg`/`libwebp` C bindings | Only if encode quality/speed at max compression becomes a product concern. C deps complicate the single-exe CI build. |
| `windows` crate | `windows-sys` 0.61.2 | If compile times or last-few-hundred-KB matter. `windows-sys` is raw `extern` declarations (no wrappers, all unsafe). Fine choice, but `windows`' ergonomics are worth it and both hit the budget. Note: `tray-icon`/`global-hotkey` internally use `windows-sys` — that coexists fine with `windows`. |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| Tauri / Electron-style shells | WebView2 alone idles >50 MB; violates every budget | Raw Win32 |
| `winit` + `softbuffer`/`pixels` for the overlay | Extra deps, event-loop ownership conflicts with tray-icon's simple pump expectation | `WS_POPUP` topmost window + GDI double-buffering (per spec) |
| `screenshots` crate 0.8.10 | Abandoned (last release Mar 2024); superseded by `xcap` by the same author | Hand-rolled BitBlt |
| `image` WebP for lossy | `WebPEncoder` is **lossless-only** (verified in docs.rs 0.25.10: "Right now only lossless encoding is supported") | Lossless WebP for MVP; `webp` 0.3.1 crate post-MVP if lossy+quality is demanded |
| WinRT `ToastNotification` APIs | Requires app identity/AUMID plumbing, pulls WinRT activation machinery into an otherwise pure-Win32 exe | Custom layered `WS_POPUP` toast window with a timer |
| `notify-rust` / `winrt-notification` | Same WinRT weight, less control over the 0%-idle constraint | Same as above |
| Default release profile | ~2–4x larger binary, panic unwinding machinery | Size profile above |
| `zopfli`/max PNG compression on hot path | Seconds-per-frame encode | `CompressionType::Fast` |

## Stack Patterns by Variant

- Add `webp = "0.3"` and gate the quality slider to JPG+WebP.
- Because `image` cannot do lossy WebP; accept the C build dependency and ~0.5 MB.
- Move capture to DXGI Desktop Duplication (via `windows` crate `Win32_Graphics_Dxgi` features) or `windows-capture` 2.0.1.
- Because GDI BitBlt tone-maps HDR poorly and can miss hardware-overlay content; this is the known BitBlt limitation and is why the spec already defers it.
- Run `cargo-bloat`; the usual suspects are `image` codec features you didn't disable, `serde_json` pretty-printing generics, or accidental `windows` feature over-inclusion.

## Version Compatibility

| Package A | Compatible With | Notes |
|-----------|-----------------|-------|
| tray-icon 0.24.2 | global-hotkey 0.8.0 | Same maintainer (Tauri), same event-receiver pattern, both happy on one raw pump thread. Create both on the pump thread. |
| tray-icon 0.24.2 | muda 0.19.3 | muda is re-exported by tray-icon; don't add muda separately unless you need its version pinned. |
| windows 0.62 | windows-sys (transitive, from tray-icon/global-hotkey) | Different crates; coexist without conflict. Don't unify — let each dep bring its own. |
| image 0.25.10 | image-webp 0.2.4 (transitive) | Lossless-only encoder, as noted. |

## Sources

- crates.io API (2026-08-18) — verified latest stable versions: windows 0.62.2, windows-sys 0.61.2, global-hotkey 0.8.0, tray-icon 0.24.2, image 0.25.10, serde 1.0.229, serde_json 1.0.151, webp 0.3.1, dirs 6.0.0, muda 0.19.3, xcap 0.9.8, screenshots 0.8.10 (stale), windows-capture 2.0.1, winit 0.30.13 — HIGH
- docs.rs/image `codecs::webp::WebPEncoder` — verified lossless-only encoding, `new_lossless` sole constructor — HIGH
- docs.rs/tray-icon — verified "on Windows, a win32 event loop" requirement (winit not required), muda integration, same-thread creation rule — HIGH
- Binary-size flag set (opt-level=z, fat LTO, panic=abort, strip) — standard min-sized-rust guidance; expected 1.5–3 MB is an estimate — MEDIUM
- GDI BitBlt HDR/overlay limitations — training knowledge, consistent with spec's own DXGI deferral note — MEDIUM

<!-- GSD:stack-end -->

<!-- GSD:conventions-start source:CONVENTIONS.md -->

## Conventions

Conventions not yet established. Will populate as patterns emerge during development.
<!-- GSD:conventions-end -->

<!-- GSD:architecture-start source:ARCHITECTURE.md -->

## Architecture

Architecture not yet mapped. Follow existing patterns found in the codebase.
<!-- GSD:architecture-end -->

<!-- GSD:skills-start source:skills/ -->

## Project Skills

No project skills found. Add skills to any of: `.claude/skills/`, `.agents/skills/`, `.cursor/skills/`, `.github/skills/`, or `.codex/skills/` with a `SKILL.md` index file.
<!-- GSD:skills-end -->

<!-- GSD:workflow-start source:GSD defaults -->

## GSD Workflow Enforcement

Before using Edit, Write, or other file-changing tools, start work through a GSD command so planning artifacts and execution context stay in sync.

Use these entry points:

- `/gsd:quick` for small fixes, doc updates, and ad-hoc tasks
- `/gsd:debug` for investigation and bug fixing
- `/gsd:execute-phase` for planned phase work

Do not make direct repo edits outside a GSD workflow unless the user explicitly asks to bypass it.
<!-- GSD:workflow-end -->

<!-- GSD:profile-start -->

## Developer Profile

> Profile not yet configured. Run `/gsd:profile-user` to generate your developer profile.
> This section is managed by `generate-claude-profile` -- do not edit manually.
<!-- GSD:profile-end -->
