# zerdr

**Keep Herdr workspace focus aligned with the matching Git checkout in Zed**

Launch Herdr from a local terminal and route each workspace to its checkout in Zed.

## Install

Install the current GitHub version with Rust 1.93.1:

```bash
cargo install --git https://github.com/ryonakae/zerdr --locked
```

## Quickstart

Install the Herdr focus hook and Zed tasks, then start the dedicated Herdr session:

```bash
zerdr setup
zerdr
```

Run `zerdr` from the terminal you want to use with Herdr. zerdr routes the focused workspace's checkout to Zed.

## Commands

| Command | Description |
|---|---|
| `zerdr` | Open or attach the `zerdr` Herdr session and choose a routing mode from the terminal environment. |
| `zerdr pick` | Choose a Herdr workspace with a fuzzy picker. |
| `zerdr next` | Focus the next workspace in Herdr display order. |
| `zerdr previous` | Focus the previous workspace in Herdr display order. |
| `zerdr sync` | Reapply the focused Herdr workspace route to Zed. |
| `zerdr bind [--session NAME] [PATH]` | Bind the selected workspace to a Git checkout; sync it when a wrapper is live. |
| `zerdr unbind [--session NAME]` | Remove the selected workspace binding. |
| `zerdr setup` | Install or update the Herdr plugin and five global Zed tasks. |
| `zerdr uninstall [--purge]` | Remove integration files; `--purge` removes zerdr state too. |
| `zerdr doctor` | Check required commands, installed files, bindings, routes, and leases. |

Bare `zerdr` accepts `--mode auto|internal|external`, `--anchor PATH`, and `--focus terminal|zed`. Launch options cannot accompany a subcommand.

`pick`, `next`, `previous`, and `sync` require a live `zerdr` wrapper and use its routing mode. `bind` and `unbind` use the current Herdr pane when available, otherwise the `zerdr` session; pass `--session NAME` to target another session. Without a live wrapper, binding changes do not route Zed.

## Routing modes

- **Internal:** Start zerdr in a Zed terminal whose current directory belongs to the target window. Each workspace change runs `zed --existing ANCHOR` followed by `zed --add TARGET`, then promotes the target checkout to the next anchor. Zed's CLI cannot verify that the starting checkout belongs to that window.
- **External:** Ghostty, iTerm, and other local terminals route the focused checkout with `zed --existing TARGET`. On macOS, zerdr tries to restore terminal focus after Zed routes the project; focus changes and Spaces can interrupt restoration. Linux leaves Zed in front.

Use `--anchor PATH` to choose the first internal anchor. Use `--focus zed` on macOS to leave Zed in front after external routing.

Zed can reuse a window that contains the target project. Its CLI cannot force a new window when no window contains that project.

## Requirements

- **Rust 1.93.1:** required for the current source installation.
- **Zed 1.15.0 or newer:** the `zed` CLI must expose `--existing` and `--add`.
- **Herdr 0.8.0 or newer:** the plugin API must expose `workspace.focused` events.
- **Local Git checkouts:** each Herdr workspace maps to one canonical checkout root.
- **Local macOS or Linux terminal:** runtime commands reject SSH, WSL, containers, and dev containers.

`zerdr doctor`, `zerdr --help`, and `zerdr --version` remain available in remote environments. Remote doctor skips runtime checks and state cleanup.

## Notes

- **Zed tasks:** `zerdr setup` adds global tasks and prints optional keybindings. It does not edit your Zed keymap.
- **Wrapper ownership:** One live `zerdr` wrapper owns routing for both modes. Stop it before changing modes.
- **Session lifetime:** zerdr keeps the Herdr session named `zerdr` after the wrapper exits, but synchronization stops with the wrapper lease.
- **Missing checkouts:** A missing checkout keeps its binding. Restore the checkout, run `zerdr bind PATH`, or remove the binding with `zerdr unbind`.
- **Uninstall:** `zerdr uninstall` keeps bindings and route state. Stop the live wrapper before running `zerdr uninstall --purge`.

## License

MIT
