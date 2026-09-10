# Changelog

All notable changes to zerdr are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.8.0

_2026-09-09_

### Changed

- **Breaking:** attached terminal threads now detach on their own and reattach when you return to the thread in Zed — selecting it in the sidebar, clicking it, or pressing a key. A thread detaches when another Herdr client selects its pane, when Zed has been in the background for two seconds (macOS), when the new `zerdr.detach-thread` Herdr action is fired on the pane from another client, or when `zerdr detach` asks every thread at once (usable over SSH before opening `herdr`). `zerdr attach`, the `zerdr: Detach` and `zerdr: Attach` Zed tasks, and their keybinding examples are gone; the next `zerdr setup install` removes the two tasks from your Zed tasks file (a copy you modified is left alone). Rerun `zerdr setup install` after upgrading: the Herdr plugin manifest gains a `pane.focused` hook and the `detach-thread` action, and `zerdr start`, `zerdr setup doctor`, and `zerdr setup auto enable` require the hooks.

- `zerdr connect` now focuses the Herdr workspace only while a `zerdr start` wrapper is live. Without follow mode the focus served nothing and, on Herdr 0.8.x where clients share one view, it parked every other client on the thread's pane, so selecting that pane there could never trigger the detach.

### Fixed

- The Herdr plugin hooks (`sync-from-herdr`, `open-from-herdr`, and the new `detach-from-herdr`) now run when the Herdr server was first started from an SSH session. Herdr passes the server's environment to plugin commands, so the SSH markers it inherited made zerdr's remote-environment check reject every hook, silently breaking focus sync and the detach hook.

## v0.7.0

_2026-08-27_

### Added

- `zerdr setup install` now adds global `zerdr: Detach` and `zerdr: Attach` Zed tasks, so attached terminal threads can be suspended and resumed from Zed's task picker without needing a free shell. The tasks run without taking focus, hide their terminal after success, and setup prints optional direct keybindings.

## v0.6.0

_2026-08-27_

### Changed

- **Breaking:** `zerdr connect` now creates and binds a missing Herdr workspace automatically, registering linked Git worktrees via `herdr worktree open`; the `--create` option has been removed. `connect` no longer starts stopped named sessions headless — launch them with `zerdr start --session NAME` so routing and focus sync are active from the start.

## v0.5.1

_2026-08-25_

### Fixed

- zerdr now removes Herdr's pane and plugin runtime context before launching Zed. A Zed process first opened by a Herdr event no longer passes `HERDR_ENV=1` to its integrated terminals, where `zerdr start` would be rejected as a nested Herdr launch.

## v0.5.0

_2026-08-25_

### Changed

- The recommended Herdr keybinding for the Open Zed action is now `prefix+shift+z`: plain `prefix+z` is Herdr's built-in pane zoom, which shadows a command binding on the same key. `zerdr setup install` prints the updated example.
- Without a live wrapper, the Open Zed action now runs `zed --existing TARGET` instead of `zed TARGET`, so a Zed window that already has the checkout open comes to the front instead of a duplicate window opening. Zed's `"cli_default_open_behavior": "new_window"` never matches an already-open project root (its subpath matching excludes worktree roots), which made the plain form duplicate windows.

## v0.4.0

_2026-08-25_

### Added

- `zerdr detach` and `zerdr attach`: suspend and resume every terminal thread's Herdr attach in one command. A direct attach pins its pane's PTY to the thread terminal's size, which breaks the session for differently sized clients; run `zerdr detach` before opening the session from a small client (for example a phone terminal over SSH) and `zerdr attach` when you are back. Both commands are global across sessions, wait until every live thread confirms (reporting counts, with a non-zero exit and the pending count on timeout), and run over SSH on the same machine — exactly where a phone-sized client needs them.
- While detached, each thread stays open in Zed, keeps its pane reserved, and keeps following the agent's title in the sidebar with a `[herdr⏸]` marker; the settle bell stays quiet. Threads opened while detach mode is on wait without attaching and connect together on `zerdr attach`.
- Reattaching goes through the pane's current terminal, so it works whether or not the agent inside changed in the meantime; a pane that no longer exists ends its thread gracefully without disturbing the others.

## v0.3.0

_2026-08-24_

### Added

- Terminal threads: `zerdr connect` attaches a Zed agent-panel terminal thread to a Herdr agent in the workspace matching the open project. Per-pane leases keep two threads off the same agent, remembered panes let restored threads reattach after a Zed restart, and with no free agent the thread gets a fresh Herdr tab holding a plain shell (`--kind` or `ZERDR_THREAD_KIND` starts an agent instead).
- Threads sidebar integration: titles carry a `[herdr]` marker with the agent's friendly name and live title, lead with a status glyph mirroring Herdr's indicators (agent-drawn spinners pass through), and the terminal bell rings when the agent settles so Zed notifies.
- Auto mode: `zerdr setup auto enable` installs `zerdr connect --auto` as Zed's `agent.terminal_init_command`, attaching every new thread best-effort — creating and binding a missing workspace, registering linked Git worktrees via `herdr worktree open` — and silently leaving a plain shell when Herdr cannot help.
- Named sessions: a global `--session` flag across `connect`, `start`, and `workspace`; `zerdr connect --create --session NAME` starts a not-running named session headless first.
- `zerdr setup doctor` points out bindings whose worktree checkout is gone and suggests `herdr worktree remove`.

### Changed

- **Breaking:** the CLI is restructured into `connect`, `start`, `workspace`, and `setup`; the external routing mode and the `pick`/`next`/`previous` commands are gone, and `setup auto` takes `enable`/`disable`.

### Fixed

- The thread title reverts to the workspace label when the attached agent exits.
- `herdr worktree open` is anchored to the repository's parent checkout.
- Setup writes through symlinked Zed configuration files instead of replacing the link.

## v0.2.0

_2026-08-20_

### Added

- Shared named Herdr sessions: zerdr can target a named session alongside the default persistent one.

## v0.1.0

_2026-08-20_

### Added

- Initial release: launch Herdr wrapped with Zed focus sync so selecting a Herdr workspace brings its Git checkout into Zed, with anchor-routed workspace synchronization, focus restoration, session-scoped bindings, an Open Zed plugin action for Herdr keybindings, and Homebrew packaging.
