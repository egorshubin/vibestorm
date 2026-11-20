# VibeZen — notes for agents

Native Windows IDE in Rust (eframe/egui 0.37, glow renderer). See README.md for features.

## Building

**Always build with `powershell -File build.ps1`, never plain `cargo build`.**
The user usually runs VibeZen itself from `target\release\vibezen.exe` (desktop
shortcut) — often while you are working inside it. A running exe is locked, so a plain
`cargo build --release` fails at the link step (LNK1104). `build.ps1` renames the locked
files aside and then builds. After a successful build, tell the user to restart VibeZen.

- `cargo check --release` is fine for quick type-checking (it doesn't touch the exe).
- Don't kill the running `vibezen.exe` — the user's projects and agents live in it.

## Testing without disturbing the user's window

Start a second instance with throw-away settings, e.g. in PowerShell:
`$env:APPDATA="$env:TEMP\vs-test"; $env:VIBEZEN_VIEW="git"; .\target\release\vibezen.exe <folder>`
(`VIBEZEN_VIEW=git` / `log` / `find` opens the Commit / Git log / Find in Files tool window on start.)

## Conventions

- Source files are UTF-8 with LF line endings. Don't rewrite them with PowerShell
  `Get-Content`/`Set-Content` (it breaks the encoding) — use the edit tools.
- User settings: `%APPDATA%\VibeZen\settings.json` (`src/settings.rs`).
