# zerdr

**Keep Herdr workspace focus aligned with the matching Git checkout in Zed**

Launch Herdr with Zed focus sync (`zerdr start`) so each workspace routes to its checkout in Zed, and connect Zed's terminal threads to the Herdr panes running in that checkout (`zerdr connect`).

## Install

Once a release is available, install with Homebrew:

```bash
brew install ryonakae/tap/zerdr
```

Or install the current GitHub version from source with Rust 1.93.1:

```bash
cargo install --git https://github.com/ryonakae/zerdr --locked
```

## Quickstart

Install the Herdr focus hook, Open Zed action, and the Zed task, then open the default Herdr session with Zed routing enabled:

```bash
zerdr setup install
zerdr start
```

`zerdr start` opens or attaches the same default persistent session as bare `herdr`. Use `zerdr start --session NAME` to open or attach the same named session as `herdr --session NAME`. Run `zerdr start` from a Zed terminal in the project you consider your anchor (or spawn the installed "zerdr: Herdr" task); selecting a workspace in Herdr then brings its checkout into that Zed window.

To keep normal Herdr workspace switching independent from Zed, do not start the wrapper. Add a configurable Herdr keybinding instead:

```toml
[[keys.command]]
key = "prefix+shift+z"
type = "plugin_action"
command = "zerdr.open-zed"
description = "open workspace in Zed"
```

`prefix+shift+z` keeps the Z-for-Zed mnemonic while staying clear of Herdr's defaults: plain `prefix+z` is Herdr's built-in pane zoom, which shadows a command binding on the same key. The action brings the current workspace's Zed window to the front — opening the checkout first when no window has it — and leaves Zed in front. `zerdr setup install` prints this example but does not edit your Herdr configuration.

## Terminal threads

Open a terminal thread in Zed's agent panel and run `zerdr connect` inside it. zerdr finds the Herdr workspace for the project you have open — by explicit binding, by Herdr's recorded checkout, or by where the workspace's panes sit, remembering that last match as a binding. If none matches, it creates and binds one before connecting the thread to a free agent there. If every agent is already attached to another thread, it opens a fresh Herdr tab holding a plain shell, the same starting point as creating a tab in Herdr; launch whatever you like there and the sidebar title and notifications follow as soon as Herdr recognizes the agent. Pass `--kind pi` (or set `ZERDR_THREAD_KIND`) to have the fresh tab start an agent immediately instead.

When zerdr connects a thread to Herdr, it prints one status line saying what the thread is attached to — an agent, a fresh tab, a created workspace, or a reattached pane — with the pane and workspace.

While attached, the threads sidebar title marks the thread as Herdr-backed and follows the agent's own live title: `[herdr] Pi - <title>` (pi's `π - ` lead is folded into the name) or `[herdr] Claude - <title>`, and `[herdr] <workspace>` for a plain shell pane. Local shells remain unmarked, so no extra terminal message is needed to distinguish them. Because the agent's own title is what follows the marker, tools that enrich agent titles with session context improve the display automatically. When the agent exits, the title falls back to `[herdr] <workspace>`, and follows again as soon as another agent starts in the pane.

Agent titles also lead with a status glyph, which Zed shows as the thread's row icon. When the agent decorates its own title with one — Claude Code animates a spinner there — that glyph is passed through unchanged. Otherwise the glyph mirrors Herdr's status symbols: `◐` working, `×` blocked, `✓` done, `○` idle, and `·` when Herdr cannot classify the agent. Plain shell panes stay unmarked. zerdr also rings the terminal bell when the agent stops working, which is what makes Zed notify you. Enable Zed's notifications to see the bell:

```json
{
  "agent": {
    "notify_when_agent_waiting": "all_screens",
    "play_sound_when_agent_done": true
  }
}
```

Because Herdr owns the session, closing a thread or restarting Zed does not stop the agent. Reopen a thread and run `zerdr connect` to attach again, or reach the same agent over SSH with `herdr` from another machine. zerdr also remembers which panes its threads were attached to: a new bare thread first takes a free agent, then reattaches a remembered live shell pane, and only then creates a fresh tab — so restarting Zed restores the previous panes instead of piling up tabs. A pane freed by closing its thread is picked up by the next thread the same way.

Attaching is manual by default, so Zed sessions that do not involve Herdr stay untouched. To attach every new terminal thread automatically, enable auto mode:

```bash
zerdr setup auto enable
```

The first `enable` writes `zerdr connect --auto` into Zed's `agent.terminal_init_command` (backing your settings file up and writing through a dotfiles symlink); after that the toggle only flips a flag in zerdr's state directory. While the mode is enabled, each new thread attaches best-effort: a project without a matching Herdr workspace gets one created and bound automatically — registered via `herdr worktree open` when it is a linked worktree — and when that cannot help — outside a Git checkout, or Herdr is not running — it silently leaves the thread as a plain local shell. Because Zed restores terminal threads on restart, restored threads reattach to the still-running agents — resume, in effect — and can likewise create workspaces for restored projects. `zerdr setup auto disable` turns the mode off without touching your Zed settings; while disabled, each new thread prints one line saying the mode is off and how to attach manually or re-enable it. `zerdr setup doctor` shows the current state. Prefer staying manual? `zerdr setup install` prints a `terminal::SendText` keybinding example for typing `zerdr connect` with one key.

When the checkout is a linked Git worktree — made by `git worktree add`, Worktrunk, Herdr itself, or any other tool — `zerdr connect` registers it with `herdr worktree open`, so the new workspace carries its checkout provenance and `herdr worktree list` and `herdr worktree remove` manage it like one Herdr created. zerdr never creates or removes worktrees itself, and if registration fails a manual connect reports Herdr's error rather than falling back to a plain workspace. After you delete a worktree externally, `zerdr setup doctor` points out the leftover binding and suggests `herdr worktree remove`.

To select text with the mouse while attached, hold Shift and drag: the Herdr client enables mouse reporting, and Shift is Zed's built-in escape hatch for native selection (fixed for the no-prior-selection case in Zed v1.16.1). Through Herdr 0.8.2 the attach client always captures the mouse and discards clicks and drags, so Shift+drag is the only way to select. Herdr has merged a fix ([herdr#2995](https://github.com/herdrdev/herdr/pull/2995), not yet in any release as of 0.8.2): once it ships, setting `ui.mouse_capture = false` in Herdr's `config.toml` makes the attach client leave the mouse to Zed, and plain drag selects natively. The setting is global — the full Herdr client then loses its own mouse UI (sidebar clicks, drag-select copy, wheel scrollback) as well, though pane apps that request the mouse, and popups, still capture it dynamically.

### Sharing the session with a small client

An attached thread pins its Herdr pane to the thread terminal's size, so opening the same session from a much smaller client — a phone terminal over SSH — shows those panes clipped to the wrong grid. Suspend every thread's attach first:

```bash
zerdr detach
```

Each thread stays open in Zed, keeps its pane reserved, and keeps following the agent's title in the sidebar with a `[herdr⏸]` marker (notifications stay quiet); the panes themselves are free to fit whichever Herdr client you use next. The command waits until every thread has confirmed, and works over SSH — run it from the phone before launching `herdr`. Back at your desk:

```bash
zerdr attach
```

Every thread reconnects to its pane, whether or not the agent inside changed in the meantime. Threads opened while detach mode is on wait the same way and connect on `zerdr attach`.

### Named sessions

`zerdr connect --session NAME` attaches to a running named Herdr session. If it is stopped, launch it with `zerdr start --session NAME`; `connect` never starts sessions itself. Keeping startup under `start` ensures the session has zerdr's route and focus sync from the beginning.

## Commands

| Command | Description |
|---|---|
| `zerdr connect [TARGET] [--session NAME]` | Connect a Zed terminal thread to a Herdr agent or a fresh shell tab, creating a workspace when needed; `TARGET` is a pane id or agent name. Add `--kind KIND` to start an agent in a fresh tab. |
| `zerdr start [--session NAME] [--anchor PATH]` | Open or attach the default or named Herdr session with Zed routing. |
| `zerdr workspace sync [--session NAME]` | Reapply the focused Herdr workspace route to Zed. |
| `zerdr workspace bind [PATH] [--session NAME]` | Bind the selected workspace to a Git checkout; sync it when a wrapper is live. |
| `zerdr workspace unbind [--session NAME]` | Remove the selected workspace binding. |
| `zerdr setup install` | Install or update the Herdr plugin and the global "zerdr: Herdr" Zed task. |
| `zerdr setup uninstall [--purge]` | Remove integration files, the owned init command, and the auto-mode flag; `--purge` removes zerdr state too. |
| `zerdr setup doctor [--session NAME]` | Check required commands, installed files, bindings, routes, leases, and auto mode for the selected session. |
| `zerdr setup auto enable\|disable` | Toggle auto mode; the first `enable` installs `zerdr connect --auto` as Zed's `agent.terminal_init_command`. |

Bare `zerdr`, `zerdr workspace`, and `zerdr setup` print their subcommands. `--session NAME` is accepted before or after the subcommand, at most once; omitting it selects Herdr's default session.

`workspace sync` requires a live zerdr wrapper for the selected session. Workspace commands use the current Herdr pane when available, otherwise the default session; pass `--session NAME` to target another session. Without a live wrapper for that session, binding changes do not route Zed. Workspace switching itself happens in Herdr's own UI.

## Routing

Start `zerdr start` in a Zed terminal whose current directory belongs to the target window, or pass `--anchor PATH`. Each workspace change runs `zed --existing ANCHOR` followed by `zed --add TARGET`, then promotes the target checkout to the next anchor. Zed's CLI cannot verify that the starting checkout belongs to that window, and zerdr does not check where it was launched from.

Zed can reuse a window that contains the target project. Its CLI cannot force a new window when no window contains that project.

The one-shot plugin action reuses an applicable live wrapper route. Without a wrapper, it runs `zed --existing TARGET`: a window that already has the checkout open comes to the front regardless of Zed's `cli_default_open_behavior` setting, and when no window has it, Zed opens the checkout with its existing-window handling — added to the active window's workspaces, or a new window when none is open. A corrupt live route is reported instead of falling back to another window.

## Requirements

- **Rust 1.93.1:** required for the current source installation.
- **Zed 1.15.0 or newer:** the `zed` CLI must expose `--existing` and `--add`.
- **Herdr 0.8.0 or newer:** the plugin API must expose `workspace.focused` events, workspace actions, and plugin-action keybindings.
- **Zed terminal threads:** `zerdr connect` needs a Zed version whose agent panel hosts terminal threads.
- **Local Git checkouts:** each Herdr workspace maps to one canonical checkout root.
- **Local macOS or Linux terminal:** runtime commands reject SSH, WSL, containers, and dev containers.

`zerdr setup doctor`, `zerdr --help`, and `zerdr --version` remain available in remote environments. Remote doctor skips runtime checks and state cleanup.

## Notes

- **Keybindings:** `zerdr setup install` adds the global Zed task and prints optional Herdr and Zed keybindings. It does not edit your Herdr config or Zed keymap.
- **Terminal thread automation:** `zerdr setup install` never writes `agent.terminal_init_command`; automation is opt-in via `zerdr setup auto enable`, which records ownership so `zerdr setup uninstall` can remove the value again (with a backup under zerdr's state directory). `zerdr setup doctor` reports the mode informationally. With the mode enabled, restored terminal threads reattach after a Zed restart; disable the mode if you want a quiet restart.
- **One thread per agent:** two terminal threads never share an agent. Attaching an agent that already has a thread fails and names the pane.
- **Wrapper ownership:** Each Herdr session can have one live zerdr wrapper. Wrappers for different named sessions can coexist.
- **Session lifetime:** Exiting a zerdr client stops synchronization for that wrapper, but the default or named Herdr session remains available to `herdr` and future zerdr clients. Only an explicit `herdr session stop` ends it.
- **Missing checkouts:** A missing checkout keeps its binding. Restore the checkout, run `zerdr workspace bind PATH`, or remove it with `zerdr workspace unbind`.
- **Uninstall:** `zerdr setup uninstall` keeps bindings and route state. Stop the live wrapper before running `zerdr setup uninstall --purge`.

## License

MIT
