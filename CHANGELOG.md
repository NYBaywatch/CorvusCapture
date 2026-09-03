# Changelog

All notable changes to Corvus Capture are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] — 2026-09-03

First public release: a lightweight Windows 11 tray screenshot utility that saves
hotkey-to-file with zero friction — no dialogs, no clipboard-first detour, auto-numbered
filenames.

### Added

- **System tray icon** with a right-click menu (Settings, Open capture folder, About, Exit),
  left-click to open Settings, and single-instance enforcement (a second launch focuses the
  existing instance)
- **Global hotkeys** `F9` (fullscreen), `Ctrl+F9` (active window), `Shift+F9` (region select),
  and `Alt+F9` (Settings), each with per-hotkey conflict notification so a single failed
  registration never disables the rest
- **Fullscreen capture** of the monitor under the cursor, pixel-accurate on multi-monitor and
  mixed-DPI setups (Per-Monitor V2 DPI awareness)
- **Active-window capture** via `GetForegroundWindow` + DWM extended frame bounds, avoiding the
  invisible drop-shadow border included by the raw window rect
- **Region-selection overlay** on a frozen, ~40% dimmed screen with drag-to-select, 8 resize
  handles, interior move, live pixel-dimension readout, and confirm/cancel via
  hotkey/Enter/double-click or Esc/outside click
- **Async save pipeline**: file numbers reserved synchronously (race-safe under rapid-fire
  captures), encoding on a background thread, atomic `.tmp`-then-rename writes, and a
  skip-never-overwrite numbering scheme where deleted numbers are never reused
- **Multiple output formats** — PNG, JPG (quality slider, 50–100), BMP, and lossless WebP
- **Optional clipboard copy**, **optional save-confirmation toast**, and **optional shutter
  sound** on every capture
- **Settings window** (raw Win32): base filename, save folder picker, format dropdown, JPG
  quality slider (JPG only), toast/clipboard/shutter toggles, and Start with Windows — every
  change applies instantly to `config.json`, no restart required
- **Start with Windows** via a self-healing, quoted HKCU Run key registration that survives
  moving the exe
- **Settings UI polish**: window position memory, dark/light theme following the OS setting
  live, a Done button, and a restyled top-center translucent save toast
- **Embedded branding**: crow tray icon, circular startup splash, and full VERSIONINFO/icon
  resource embedding in the shipped exe
