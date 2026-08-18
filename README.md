<div align="center">

# zerdr

**Keep Herdr workspace focus aligned with one Zed window**

Launch a dedicated Herdr session from a project-backed Zed window, then add and activate matching Git checkouts in that window.

</div>

## Install

```bash
brew install ryonakae/tap/zerdr
```

zerdr supports macOS and Linux on arm64 and x86_64.

## Quickstart

```bash
# Install the Herdr plugin and five global Zed tasks.
zerdr setup
```

Open a Git project in Zed, run **task: spawn** from the command palette, and select `zerdr: Herdr`. The task opens or attaches the persistent Herdr session named `zerdr` and uses the current Zed project to identify its window.

You can launch it directly from a Zed integrated terminal by specifying a Git project that is already open in that window:

```bash
zerdr herdr --anchor /path/to/open/project
```

Focusing a Herdr workspace adds its canonical Git checkout to the managed Zed window and activates it.

## Commands

| Command | Description |
|---|---|
| `zerdr herdr --anchor PATH` | Open or attach the dedicated Herdr session using an existing Zed project as the initial window anchor. |
| `zerdr pick` | Fuzzily select a Herdr workspace. |
| `zerdr next` | Focus the next workspace in Herdr display order, with wrapping. |
| `zerdr previous` | Focus the previous workspace, with wrapping. |
| `zerdr sync` | Reapply the focused Herdr workspace to Zed. |
| `zerdr bind [PATH]` | Bind the focused workspace to the containing Git checkout and sync it. |
| `zerdr unbind` | Remove the focused workspace binding. |
| `zerdr setup` | Install or update the Herdr plugin and global Zed tasks. |
| `zerdr uninstall [--purge]` | Remove integration files; `--purge` also removes zerdr state. |
| `zerdr doctor` | Check executables, capabilities, tasks, plugin state, bindings, and leases. |

## Zed tasks

`zerdr setup` adds these global task labels and prints keybinding examples without editing your keymap:

- `zerdr: Herdr`
- `zerdr: Pick Workspace`
- `zerdr: Next Workspace`
- `zerdr: Previous Workspace`
- `zerdr: Sync Workspace`

The Herdr task stays visible for the life of its terminal, and zerdr admits only one live wrapper. A competing task exits in its own terminal without replacing the active Herdr UI. The picker opens in the center. Navigation and sync tasks use a non-focused terminal and close it after a successful command or delivered Herdr notification. If no live Herdr client can receive an error, the task terminal stays open with recovery instructions.

## Requirements

- **Zed 1.15.0 or newer:** the CLI must expose `zed --existing` and `zed --add`; integrated terminals must set `ZED_TERM=true` and `TERM_PROGRAM=zed`.
- **Herdr 0.8.0 or newer:** plugin events must expose `workspace.focused` with protocol 19-compatible workspace and snapshot responses.
- **Local Git checkouts:** each Herdr workspace maps to one canonical checkout root. Linked worktrees remain distinct by checkout path.

Run `zerdr doctor` after setup to check the installed commands and generated files.

## Notes

- Start `zerdr: Herdr` from a Zed window that already contains a Git project. Blank windows cannot be addressed through the Zed CLI.
- Before launch, close other Zed windows that contain projects you will select in Herdr. Zed focuses the other window when a target project is already open there, and zerdr cannot detect or move it.
- zerdr permits one live wrapper. Starting `zerdr: Herdr` again launches a candidate that zerdr rejects without replacing the existing wrapper. Keep the dedicated `zerdr` Herdr client inside the managed Zed window.
- After each successful switch, the selected project becomes the window anchor. Switch to another Herdr workspace before removing the current anchor from Zed. If you remove it first, stop the wrapper and relaunch `zerdr: Herdr` from a remaining project.
- zerdr does not synchronize project order, remove projects when Herdr workspaces close, or collapse project-panel entries.
- A missing checkout keeps its stored binding. Restore it, run `zerdr bind PATH`, or remove it with `zerdr unbind`.
- `zerdr uninstall` keeps workspace bindings and route state. Use `zerdr uninstall --purge` after closing the live wrapper to remove state.

## License

MIT
