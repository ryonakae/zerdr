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

Install the Herdr focus hook, Open Zed action, and Zed tasks, then open the default Herdr session with Zed routing enabled:

```bash
zerdr setup
zerdr
```

Bare `zerdr` opens or attaches the same default persistent session as bare `herdr`. Use `zerdr --session NAME` to open or attach the same named session as `herdr --session NAME`. Run zerdr from the terminal you want to use with Herdr; it routes the focused workspace's checkout to Zed.

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

While attached, zerdr mirrors the agent's name and Herdr terminal title into the threads sidebar and rings the terminal bell when the agent stops working, which is what makes Zed notify you. Enable Zed's notifications to see it:

```json
{
  "agent": {
    "notify_when_agent_waiting": "all_screens",
    "play_sound_when_agent_done": true
  }
}
```

Because Herdr owns the session, closing a thread or restarting Zed does not stop the agent. Reopen a thread and run `zerdr thread` to attach again, or reach the same agent over SSH with `herdr` from another machine.

Attaching is deliberately manual, so Zed sessions that do not involve Herdr stay untouched. If you want every new terminal thread attached automatically, set Zed's `agent.terminal_init_command` to `zerdr thread` yourself; `zerdr setup` prints the exact value and a `terminal::SendText` keybinding example for typing the command with one key.

Threads only attach to workspaces Herdr already manages. In a checkout without one, `zerdr thread` says so rather than adding a workspace. Run `zerdr thread --create` there when you do want the workspace.

## Commands

| Command | Description |
|---|---|
| `zerdr [--session NAME]` | Open or attach the default or named Herdr session and choose a routing mode from the terminal environment. |
| `zerdr [--session NAME] pick` | Choose a Herdr workspace with a fuzzy picker. |
| `zerdr [--session NAME] next` | Focus the next workspace in Herdr display order. |
| `zerdr [--session NAME] previous` | Focus the previous workspace in Herdr display order. |
| `zerdr [--session NAME] sync` | Reapply the focused Herdr workspace route to Zed. |
| `zerdr [--session NAME] bind [PATH]` | Bind the selected workspace to a Git checkout; sync it when a wrapper is live. |
| `zerdr [--session NAME] unbind` | Remove the selected workspace binding. |
| `zerdr [--session NAME] thread [TARGET]` | Attach a Zed terminal thread to a Herdr agent or a fresh shell tab; `TARGET` is a pane id or agent name. Add `--kind KIND` to start an agent in the fresh tab, or `--create` to allow creating the workspace. |
| `zerdr setup` | Install or update the Herdr plugin and five global Zed tasks. |
| `zerdr uninstall [--purge]` | Remove integration files; `--purge` removes zerdr state too. |
| `zerdr [--session NAME] doctor` | Check required commands, installed files, bindings, routes, and leases for the selected session. |

Bare `zerdr` accepts `--session NAME`, `--mode auto|internal|external`, `--anchor PATH`, and `--focus terminal|zed`. Omitting `--session` selects Herdr's default session. Launch options other than `--session` cannot accompany a subcommand.

`pick`, `next`, `previous`, and `sync` require a live zerdr wrapper for the selected session and use its routing mode. Manual commands use the current Herdr pane when available, otherwise the default session; pass `--session NAME` to target another session. Without a live wrapper for that session, binding changes do not route Zed.

## Routing modes

- **Internal:** Start zerdr in a Zed terminal whose current directory belongs to the target window. Each workspace change runs `zed --existing ANCHOR` followed by `zed --add TARGET`, then promotes the target checkout to the next anchor. Zed's CLI cannot verify that the starting checkout belongs to that window.
- **External:** Ghostty, iTerm, and other local terminals route the focused checkout with `zed --existing TARGET`. On macOS, zerdr tries to restore terminal focus after Zed routes the project; focus changes and Spaces can interrupt restoration. Linux leaves Zed in front.

Use `--anchor PATH` to choose the first internal anchor. Use `--focus zed` on macOS to leave Zed in front after external routing.

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

- **Keybindings:** `zerdr setup` adds global Zed tasks and prints optional Herdr and Zed keybindings. It does not edit your Herdr config or Zed keymap.
- **Terminal thread automation:** setup never writes `agent.terminal_init_command`; automation is opt-in and yours to configure. An init command that an older zerdr installed is migrated away by the next `zerdr setup` or `zerdr uninstall` (with a backup under zerdr's state directory), and `zerdr doctor` reports the setting informationally.
- **One thread per agent:** two terminal threads never share an agent. Attaching an agent that already has a thread fails and names the pane.
- **Wrapper ownership:** Each Herdr session can have one live zerdr wrapper. Stop that session's wrapper before changing its routing mode. Wrappers for different named sessions can coexist.
- **Session lifetime:** Exiting a zerdr client stops synchronization for that wrapper, but the default or named Herdr session remains available to `herdr` and future zerdr clients.
- **Missing checkouts:** A missing checkout keeps its binding. Restore the checkout, run `zerdr [--session NAME] bind PATH`, or remove it with `zerdr [--session NAME] unbind`.
- **Uninstall:** `zerdr uninstall` keeps bindings and route state. Stop the live wrapper before running `zerdr uninstall --purge`.

## License

MIT
