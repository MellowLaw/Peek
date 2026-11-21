# Peek

**Peek** is a Windows 10 and 11 Dynamic Island-style desktop notch utility built with **Tauri 2 (Rust)** and **React / TypeScript**. It docks quietly at the top center of your monitor with an opaque black pill (`Idle`), dynamically expands on hover or hotkey (`Ctrl+Alt+Space`), and puts everyday desktop tools one glance away without stealing your active window focus.

---

## Download & Install

1. Open [Latest Release](https://github.com/MellowLaw/Peek/releases/latest) and expand **Assets** at the bottom.
2. Click **`Peek_<version>_x64-setup.exe`** to download the Windows installer.
   *Ignore `latest.json`, `.sig`, and `Source code` unless you are building from source — Peek uses those internally for updates.*
3. Double-click the downloaded `.exe` installer. If Windows Defender SmartScreen shows *"Windows protected your PC"*, click **More info → Run anyway**.
4. **Automatic updates**: Once installed, Peek checks for signed releases automatically on launch and lets you click **Update now** or **Later** right from the notch (or **Settings → Notch → Updates**).

Requirements: Windows 10 or 11 (x64) with Microsoft Edge WebView2 Runtime (built into Windows 11). Optional screen recording uses `ffmpeg.exe` (`winget install Gyan.FFmpeg`).

---

## Features

- **Top-Center Dynamic Island Shell**
  - Fixed transparent host window clipped cleanly with native Win32 `SetWindowRgn` (`HRGN`) concave top ears, `Idle`, `Hover`, `Lyrics`, and auto-sizing `Expanded` modes, `Always visible` / `Show on hover` top-edge watcher, monitor picker, size presets (`Compact` / `Standard` / `Large`), and custom accent color.
  - Preserves your foreground application focus on hover and uses intentional expand / auto-close rules.
- **Music & Built-in Synced Karaoke Lyrics**
  - Reads Windows GSMTC media sessions (metadata, album cover art with disk caching & iTunes fallback, timeline scrubbing, play/pause/skip, and multi-session switching).
  - Standalone built-in **LRCLIB + AMLL TTML** synced lyrics engine with blurred artwork backdrop, word-by-word karaoke highlighting, and dynamic-width single-line collapsed Lyrics bar (no external app required; also compatible with an optional local **Lyrical** server).
- **Files Shelf (3-Column Full-Bleed Grid)**
  - Dedicated non-focus-stealing Win32 `IDropTarget` catcher (`PeekDropCatcherWindow`) accepts any file or folder dropped onto the top notch or open shelf from Explorer.
  - Full-bleed Windows Explorer thumbnails & image previews with hover-revealed single-row actions: copy/move to folder, inline rename, PNG/JPEG/WebP conversion, ZIP extraction, native Win32 OLE Move/Copy drag-out (`Drag` / `Drag all`), and time-limited LAN **QR phone transfer**.
- **Screen Capture, OCR & Recording**
  - Frozen virtual-desktop capture overlay for region, fullscreen, or active window screenshots (`Pictures\Peek` + automatic clipboard copy), Windows WinRT `OcrEngine` text recognition, and live pixel HEX/RGB color sampling.
  - External `ffmpeg.exe` (`gdigrab` + optional `dshow` mic) pause/resume segment screen recording (`Videos\Peek`) with a compact red indicator docked cleanly in the pill.
- **AI Limit Monitor**
  - Read-only local telemetry for **Claude**, **Gemini / Antigravity**, and **Codex** displaying **Weekly Limit Remaining** and **Five Hour Limit Remaining** circular rings, opt-in Claude Code `statusLine` live bridge, configurable token budgets, and pinnable minimized notch rings.
- **Clipboard History (2-Column Cards)**
  - Opt-in Windows `WM_CLIPBOARDUPDATE` listener capturing copied text and images (`PNG`, `CF_DIBV5`, `CF_DIB` — including `Win+Shift+S`, `PrintScreen`, browser **Copy image**, and Peek screenshots) into a 2-column grid with search, pinning, and one-click re-copy.
- **System Controls, Timer, Notes, Launcher, Alerts & Local API**
  - Core Audio master + per-app volume mixer, audio output switcher, WMI + DDC/CI display brightness, live CPU/RAM/Battery/Network telemetry, and window snap helpers.
  - Focus countdown timer & reminders, Notes & Snippets, `Ctrl+K` Start Menu app & action launcher, priority Notification Hub with Do Not Disturb, and an opt-in `127.0.0.1` Bearer-authenticated Local API.

---

## Privacy & Security Design

- **Strict opt-in tracking**: Clipboard history tracking, Windows NotificationListener access, Local API server, external `ffmpeg.exe` recording, and the Claude `statusLine` hook are disabled until explicitly enabled.
- **Zero credential scraping**: Peek never reads authentication tokens, session cookies, or keychains from external AI tools or browsers.
- **Signed updates & least-privilege runtime**: Tauri CSP restricts webview resources to `'self'` and `data:`/`ipc:`; capabilities grant minimal event listeners; updates from GitHub Releases are Ed25519 signature-verified before installation; optional Local API binds `127.0.0.1` only with constant-time `Authorization: Bearer` validation and rate limiting; LAN QR transfers use cryptographically random routes with 5-minute TTLs, read timeouts, and sanitized filenames.

---

## Tech Stack

- **Native Core**: Rust + Tauri 2 (`windows` / `windows-sys` Win32/WinRT/COM/DWM/Core Audio/GDI, `tauri-plugin-updater`, `tokio`, `image`, `zip`, `qrcode`)
- **Frontend**: React 19, TypeScript, Vite, Tailwind CSS v4, Motion, Lucide icons

---

## Local Development & Publishing Releases

Requires **Windows 10 / 11**, **Node.js**, **Rust**, **Microsoft Edge WebView2 Runtime**, and the **Windows C++ Build Tools** ([Tauri Windows prerequisites](https://v2.tauri.app/start/prerequisites/)).

```powershell
# Install frontend dependencies
npm install

# Run in development mode
npm run tauri dev

# Verify frontend & Rust checks
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
cargo test --manifest-path src-tauri/Cargo.toml
```

### Publishing a New Release

1. Update `"version"` in `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.
2. Ensure repository secret `TAURI_SIGNING_PRIVATE_KEY` (and optional `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) is configured in GitHub Actions settings.
3. Commit and push a matching version tag:
   ```powershell
   git tag v0.1.0
   git push origin v0.1.0
   ```
   `.github/workflows/release.yml` automatically builds, tests, signs, and publishes `Peek_<version>_x64-setup.exe` plus `latest.json` for in-app auto-updates.
