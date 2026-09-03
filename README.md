# Corvus Capture

**Hotkey-to-file screenshots, zero friction.**

> Press a key, get a correctly-named, auto-numbered file — no dialogs, no clipboard-first
> detour, no bloat. Corvus Capture is a lightweight Windows 11 tray utility built for people
> who screenshot constantly and just want the file to appear.

![Platform](https://img.shields.io/badge/platform-Windows-0078D6)
![Rust](https://img.shields.io/badge/Rust-stable-orange)
![License](https://img.shields.io/badge/license-MIT-green)

---

## Who it's for

- 🧑‍💻 Developers and support engineers who screenshot constantly while documenting bugs, steps, or tasks
- 🪶 Anyone who wants a screenshot tool that idles at effectively zero cost, not a heavyweight suite
- 🔢 People who like `task001.png`, `task002.png`… sequential naming, without renaming anything by hand
- 🚫 Anyone tired of ShareX/Snipping Tool/Greenshot's save dialogs, clipboard-first flows, or idle overhead

## Features

- 🖥️ **Fullscreen capture (`F9`)** — grabs the entire monitor under the cursor, no UI, saved immediately
- 🪟 **Active-window capture (`Ctrl+F9`)** — captures only the focused window, using DWM extended frame bounds (not the raw window rect, which includes the invisible drop-shadow border)
- ✂️ **Region capture (`Shift+F9`)** — freezes the screen under a ~40% dimmed overlay; drag to select, resize via 8 handles, move by dragging the interior, with a live pixel-dimension readout; confirm with `Shift+F9`/`Enter`/double-click, cancel with `Esc`/an outside click
- ⚙️ **Settings (`Alt+F9`)** — raw Win32 window for base filename, save folder, output format, JPG quality, toast/clipboard/shutter-sound toggles, and Start with Windows — every change applies instantly, no restart
- 🔢 **Auto-numbering** — `{base}{NNN}.{ext}`, zero-padded to 3 digits and growing past 999 (`task1000.png`); collisions are always skipped, never overwritten, and deleted numbers are never reused
- 🖼️ **Multiple formats** — PNG, JPG (quality slider, 50–100), BMP, and lossless WebP
- 📋 **Optional clipboard copy** — toggle in Settings; the file save always happens regardless
- 🔔 **Optional save toast** — a small on-screen confirmation ("Saved task004.png"), toggleable
- 🔊 **Optional shutter sound** — a camera-shutter cue on capture, default off
- 🚀 **Start with Windows** — self-healing quoted Run key registration, survives moving the exe
- 🎯 **Per-Monitor V2 DPI awareness** — pixel-accurate captures on mixed-DPI multi-monitor setups
- 🐦‍⬛ **Crow tray icon + circular startup splash** — lives quietly in the system tray between captures

Settings persist to `%APPDATA%\CorvusCapture\config.json` and are re-read live, so hand edits to
the file apply immediately with no restart.

## Footprint

Measured from a clean `cargo build --release` (see repository `05-FOOTPRINT.md` methodology:
`Get-Counter '\Process(CorvusCapture)\Working Set - Private'`, 65s+ idle settle, 3 samples).

| Metric | Measured | Budget | Method |
|--------|----------|--------|--------|
| Exe size | 1.13 MB (1,182,720 bytes) | < 5 MB | `cargo build --release`, direct file size |
| Idle private working set | 1.42 MB (max of 3 samples) | < 10 MB | `Get-Counter` on the live process, 65s+ settled idle |
| Idle CPU | 0.00% | ~0% | `Get-Counter '\Process(CorvusCapture)\% Processor Time'`, 3 samples |

No DLLs ship alongside the exe — it is a fully standalone, single portable file.

## Known limitations

- **Unsigned binary** — Windows SmartScreen may show "Windows protected your PC" on first run.
  Click **More info**, then **Run anyway** to launch. Verify the download against the SHA-256
  checksum published on the [release](https://github.com/NYBaywatch/CorvusCapture/releases) page
  before doing so.
- **No HDR tone-mapping** — capture uses GDI BitBlt, which does not tone-map HDR content
  correctly and may miss hardware-overlay content. DXGI Desktop Duplication is deferred
  post-MVP.
- **WebP is lossless only** — the `image` crate's WebP encoder has no lossy mode, so the JPG
  quality slider applies to JPG only.
- **Rounded window corners** — Windows 11's rounded window corners mean active-window captures
  include background pixels in the corners, matching Win+PrintScreen's behavior.
- **Fixed hotkeys** — `F9`, `Ctrl+F9`, `Shift+F9`, and `Alt+F9` are not rebindable in v0.1
  (rebinding is planned). Note: `F9` triggers recalculation in Excel and field updates in Word.
- **Windows 11 x64 only** — no other platforms are supported.

## Architecture

```
Tray icon + global hotkeys (raw Win32 message pump)
        │
        ├── F9 ────────► fullscreen capture (BitBlt, monitor under cursor)
        ├── Ctrl+F9 ───► active-window capture (DWM extended frame bounds)
        ├── Shift+F9 ──► frozen-screen overlay ──► crop selection
        └── Alt+F9 ────► Settings window (raw Win32, instant apply)
                                │
                                ▼
                     async save pipeline
        (number reserved synchronously, encode on background
         thread, atomic .tmp-then-rename write, PNG/JPG/BMP/WebP)
                                │
                                ▼
                  {base}{NNN}.{ext} on disk
```

## Design highlights

- Raw Win32 message pump instead of `winit` — smallest binary and idle-RAM footprint for a
  Windows-only tray app
- `tray-icon` + `global-hotkey` (Tauri-maintained) run on the same raw pump thread — no extra
  windowing framework required
- GDI BitBlt capture, hand-rolled — full control over DPI and multi-monitor handling in ~40 lines
- Numbers are reserved synchronously before async encode, so rapid-fire captures can never
  collide or overwrite
- Size-optimized release profile (`opt-level = "z"`, fat LTO, single codegen unit, panic=abort,
  stripped symbols) keeps the exe under 1.2 MB

## Build & run

```sh
cargo build --release
```

The resulting binary is at `target/release/CorvusCapture.exe`.

Pre-built Windows binaries are attached to each
[release](https://github.com/NYBaywatch/CorvusCapture/releases).

## License

MIT — see [LICENSE](LICENSE).
