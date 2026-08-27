# Changelog

All notable changes to zerdr are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
