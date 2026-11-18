# ⚡ VibeStorm

A lightweight native IDE for Windows, written in Rust (egui + OpenGL). It has a PhpStorm-style layout, git, and one **Claude Code agent per project, all running in parallel**.

- Single 8 MB executable. Idle CPU is 0% because it only repaints on input or while an agent is working.
- Measured on this laptop: about 100 MB working set and 10 threads. Most of that is the NVIDIA OpenGL driver.

## Run / build

```
target\release\vibestorm.exe [project-folder]
cargo build --release
```

Settings are stored in `%APPDATA%\VibeStorm\settings.json`. Open projects, their tabs and each project's last Claude session are restored on start.

## Layout

| Area | What it is |
|---|---|
| Far-left sidebar | **Projects**. Open projects are at the top in load order, with the folder colour showing the agent: 🟦 idle · 🟨 working (pulses) · 🟩 finished · 🟥 needs you (approval / question) or error. **RECENT** projects (grey folders) are sorted by their last action, newest first. Hover a row to get 🗀 files · ⎇ git · ⏏ unload from memory (stops the agent, moves the project to RECENT). |
| Left stripe | Project tree (Alt+1), Commit (Alt+0, JetBrains-style change tree), Git log (Alt+9), Find in project (Ctrl+Shift+F: search as you type, Aa / whole word / regex, file mask, ↑↓ + Enter) |
| Center | Editor tabs. Syntax highlighting (Sublime grammars, incremental), line numbers, find, image preview |
| Right | Claude Code panel (Alt+2), styled after the VS Code extension. It opens by default for every project; 🕘 lists past conversations |
| Toolbar | Project menu, branch switcher, Fetch / Pull / Commit / Push, terminal, light/dark theme, settings |

## Claude Code

Each project runs its own `claude` process (stream-json protocol, the same one the Agent SDK uses), so every Claude Code capability works: tools, MCP servers, skills, hooks, subagents, slash commands, plan mode and session resume.

- **Approvals:** Allow / Always allow / Deny cards, with a diff preview for edits. There are also cards for Claude's questions and for plan approval.
- **Modes:** Ask, Accept edits, Auto, Plan, Bypass. **Models:** Default, Fable, Opus, Sonnet, Haiku.
- **Images:** 📎 attach, **Ctrl+V** paste a screenshot, or drag an image file onto the window.
- **Notifications:** when a task finishes or needs you, in any project: a Windows toast from "VibeStorm" (click it to jump to the project), a card in the bottom-right of the IDE, and a taskbar flash. You can change this in ⚙ Settings.
- Files Claude edits reload in open tabs automatically, and git status refreshes.
- Commit message ✨ Generate uses a one-shot Claude Haiku call on the selected diff.
- `claude` processes run in a Windows Job Object, so they die with the IDE even if it crashes.

## Shortcuts (PhpStorm keymap)

| Keys | Action |
|---|---|
| Ctrl+Shift+N | Go to file |
| Ctrl+Shift+F | Find in project (selected text becomes the query) |
| Ctrl+F / Ctrl+G | Find in file / go to line |
| Ctrl+S | Save all |
| Ctrl+W, Ctrl+F4 | Close tab |
| Ctrl+Tab | Next tab |
| Ctrl+K / Ctrl+Enter | Commit window / commit |
| Ctrl+Shift+K | Push |
| Ctrl+T | Pull |
| Ctrl+Alt+[ / ] | Previous / next project |
| Ctrl+= / Ctrl+- | Editor font size |

## Not included yet

Integrated terminal (the `>_` button opens Windows Terminal), code completion / LSP, debugger, side-by-side diff, multi-cursor.
