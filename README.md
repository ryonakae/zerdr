# zerdr

**Keep Herdr workspace focus aligned with the matching Git checkout in Zed**

Launch Herdr from a local terminal and route each workspace to its checkout in Zed, and attach Zed's terminal threads to the Herdr agents running in that checkout.

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
zerdr setup
zerdr
```

Bare `zerdr` opens or attaches the same default persistent session as bare `herdr`. Use `zerdr --session NAME` to open or attach the same named session as `herdr --session NAME`. Run zerdr from a Zed terminal in the project you consider your anchor (or spawn the installed "zerdr: Herdr" task); selecting a workspace in Herdr then brings its checkout into that Zed window.

To keep normal Herdr workspace switching independent from Zed, do not start the bare wrapper. Add a configurable Herdr keybinding instead:

```toml
[[keys.command]]
key = "prefix+z"
type = "plugin_action"
command = "zerdr.open-zed"
description = "open workspace in Zed"
```

The action opens the current workspace in Zed once and leaves Zed in front. `zerdr setup` prints this example but does not edit your Herdr configuration.

## Terminal threads

Open a terminal thread in Zed's agent panel and run `zerdr thread` inside it. zerdr finds the Herdr workspace for the project you have open — by explicit binding, by Herdr's recorded checkout, or by where the workspace's panes sit, remembering that last match as a binding — and attaches the thread to a free agent there. If every agent is already attached to another thread, it opens a fresh Herdr tab holding a plain shell, the same starting point as creating a tab in Herdr; launch whatever you like there and the sidebar title and notifications follow as soon as Herdr recognizes the agent. Pass `--kind pi` (or set `ZERDR_THREAD_KIND`) to have the fresh tab start an agent immediately instead.

Every start prints one status line saying what the thread is connected to — an attached agent, a fresh Herdr tab, a created workspace, a reattached pane, or (with auto mode off) a plain local shell — with the pane and workspace, so a Herdr pane and a local shell are distinguishable at a glance.

While attached, the threads sidebar title marks the thread as Herdr-backed and follows the agent's own live title: `[herdr] Pi - <title>` (pi's `π - ` lead is folded into the name) or `[herdr] Claude - <title>`, and `[herdr] <workspace>` for a plain shell pane. Because the agent's own title is what follows the marker, tools that enrich agent titles with session context improve the display automatically.

Agent titles also lead with a status glyph, which Zed shows as the thread's row icon. When the agent decorates its own title with one — Claude Code animates a spinner there — that glyph is passed through unchanged. Otherwise the glyph mirrors Herdr's status symbols: `◐` working, `×` blocked, `✓` done, `○` idle, and `·` when Herdr cannot classify the agent. Plain shell panes stay unmarked. zerdr also rings the terminal bell when the agent stops working, which is what makes Zed notify you. Enable Zed's notifications to see the bell:

```json
{
  "agent": {
    "notify_when_agent_waiting": "all_screens",
    "play_sound_when_agent_done": true
  }
}
```

Because Herdr owns the session, closing a thread or restarting Zed does not stop the agent. Reopen a thread and run `zerdr thread` to attach again, or reach the same agent over SSH with `herdr` from another machine. zerdr also remembers which panes its threads were attached to: a new bare thread first takes a free agent, then reattaches a remembered live shell pane, and only then creates a fresh tab — so restarting Zed restores the previous panes instead of piling up tabs. A pane freed by closing its thread is picked up by the next thread the same way.

Attaching is manual by default, so Zed sessions that do not involve Herdr stay untouched. To attach every new terminal thread automatically, turn on thread auto mode:

```bash
zerdr thread --enable
```

The first `--enable` writes `zerdr thread --auto` into Zed's `agent.terminal_init_command` (backing your settings file up and writing through a dotfiles symlink); after that the toggle only flips a flag in zerdr's state directory. While the mode is on, `--auto` attaches each new thread best-effort: a project without a matching Herdr workspace gets one created and bound automatically, and when that cannot help — outside a Git checkout, or Herdr is not running — it prints one line and leaves the thread as a plain local shell. Because Zed restores terminal threads on restart, restored threads reattach to the still-running agents — resume, in effect — and can likewise create workspaces for restored projects. `zerdr thread --disable` turns the mode off without touching your Zed settings, and `zerdr doctor` shows the current state. Prefer staying manual? `zerdr setup` prints a `terminal::SendText` keybinding example for typing `zerdr thread` with one key.

Manual `zerdr thread` only attaches to workspaces Herdr already manages. In a checkout without one, it says so rather than adding a workspace; run `zerdr thread --create` there when you do want the workspace.

To select text with the mouse while attached, hold Shift and drag: the Herdr client enables mouse reporting, and Shift is Zed's built-in escape hatch for native selection (fixed for the no-prior-selection case in Zed v1.16.1).

## Commands

| Command | Description |
|---|---|
| `zerdr [--session NAME] [--anchor PATH]` | Open or attach the default or named Herdr session with Zed routing. |
| `zerdr [--session NAME] sync` | Reapply the focused Herdr workspace route to Zed. |
| `zerdr [--session NAME] bind [PATH]` | Bind the selected workspace to a Git checkout; sync it when a wrapper is live. |
| `zerdr [--session NAME] unbind` | Remove the selected workspace binding. |
| `zerdr [--session NAME] thread [TARGET]` | Attach a Zed terminal thread to a Herdr agent or a fresh shell tab; `TARGET` is a pane id or agent name. Add `--kind KIND` to start an agent in the fresh tab, or `--create` to allow creating the workspace. |
| `zerdr thread --enable` / `--disable` | Toggle thread auto mode; `--enable` installs `zerdr thread --auto` as Zed's `agent.terminal_init_command` once. |
| `zerdr setup` | Install or update the Herdr plugin and the global "zerdr: Herdr" Zed task. |
| `zerdr uninstall [--purge]` | Remove integration files, the owned init command, and the auto-mode flag; `--purge` removes zerdr state too. |
| `zerdr [--session NAME] doctor` | Check required commands, installed files, bindings, routes, leases, and thread auto mode for the selected session. |

Bare `zerdr` accepts `--session NAME` and `--anchor PATH`. Omitting `--session` selects Herdr's default session. `--anchor` cannot accompany a subcommand.

`sync` requires a live zerdr wrapper for the selected session. Manual commands use the current Herdr pane when available, otherwise the default session; pass `--session NAME` to target another session. Without a live wrapper for that session, binding changes do not route Zed. Workspace switching itself happens in Herdr's own UI.

## Routing

Start zerdr in a Zed terminal whose current directory belongs to the target window, or pass `--anchor PATH`. Each workspace change runs `zed --existing ANCHOR` followed by `zed --add TARGET`, then promotes the target checkout to the next anchor. Zed's CLI cannot verify that the starting checkout belongs to that window, and zerdr does not check where it was launched from.

Zed can reuse a window that contains the target project. Its CLI cannot force a new window when no window contains that project.

The one-shot plugin action reuses an applicable live wrapper route. Without a wrapper, it runs `zed TARGET`, so window placement follows Zed's `cli_default_open_behavior` setting. A corrupt live route is reported instead of falling back to another window.

## Requirements

- **Rust 1.93.1:** required for the current source installation.
- **Zed 1.15.0 or newer:** the `zed` CLI must expose `--existing` and `--add`.
- **Herdr 0.8.0 or newer:** the plugin API must expose `workspace.focused` events, workspace actions, and plugin-action keybindings.
- **Zed terminal threads:** `zerdr thread` needs a Zed version whose agent panel hosts terminal threads.
- **Local Git checkouts:** each Herdr workspace maps to one canonical checkout root.
- **Local macOS or Linux terminal:** runtime commands reject SSH, WSL, containers, and dev containers.

`zerdr doctor`, `zerdr --help`, and `zerdr --version` remain available in remote environments. Remote doctor skips runtime checks and state cleanup.

## Notes

- **Keybindings:** `zerdr setup` adds the global Zed task and prints optional Herdr and Zed keybindings. It does not edit your Herdr config or Zed keymap.
- **Terminal thread automation:** `zerdr setup` never writes `agent.terminal_init_command`; automation is opt-in via `zerdr thread --enable`, which records ownership so `zerdr uninstall` can remove the value again (with a backup under zerdr's state directory). `zerdr doctor` reports the mode informationally. With the mode on, restored terminal threads reattach after a Zed restart; turn the mode off if you want a quiet restart.
- **One thread per agent:** two terminal threads never share an agent. Attaching an agent that already has a thread fails and names the pane.
- **Wrapper ownership:** Each Herdr session can have one live zerdr wrapper. Wrappers for different named sessions can coexist.
- **Session lifetime:** Exiting a zerdr client stops synchronization for that wrapper, but the default or named Herdr session remains available to `herdr` and future zerdr clients.
- **Missing checkouts:** A missing checkout keeps its binding. Restore the checkout, run `zerdr [--session NAME] bind PATH`, or remove it with `zerdr [--session NAME] unbind`.
- **Uninstall:** `zerdr uninstall` keeps bindings and route state. Stop the live wrapper before running `zerdr uninstall --purge`.

## License

MIT
