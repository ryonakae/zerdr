<div align="center">

# zerdr

**Keep Herdr workspace focus aligned with Zed**

Launch one Herdr session from Zed, Ghostty, iTerm, or another local terminal. Workspace focus changes activate the matching Git checkout in Zed.

</div>

## Install

```bash
brew install ryonakae/tap/zerdr
zerdr setup
```

Run zerdr from the terminal where you want to use Herdr:

```bash
zerdr
```

zerdr selects a routing mode from the terminal environment:

- A Zed integrated terminal uses **internal mode**. The current Git checkout anchors one managed Zed window. Workspace changes run `zed --existing ANCHOR`, then `zed --add TARGET`.
- Ghostty, iTerm, and other local terminals use **external mode**. Startup and each later workspace focus event run `zed --existing TARGET` for the focused workspace.

`zerdr setup` installs the Herdr focus hook and five optional global Zed tasks. Run setup again after replacing or moving the zerdr executable.

## Commands

| Command | Description |
|---|---|
| `zerdr` | Open or attach the dedicated Herdr session and select routing automatically. |
| `zerdr --mode internal` | Force internal routing from the current Git checkout. |
| `zerdr --mode external` | Force external routing. |
| `zerdr --anchor PATH` | Force internal routing with a canonical Git checkout anchor. |
| `zerdr --mode external --focus terminal\|zed` | Keep the terminal or Zed foreground after external routing. |
| `zerdr pick` | Fuzzily select a Herdr workspace. |
| `zerdr next` | Focus the next workspace in Herdr display order, with wrapping. |
| `zerdr previous` | Focus the previous workspace, with wrapping. |
| `zerdr sync` | Reapply the focused Herdr workspace to Zed. |
| `zerdr bind [PATH]` | Bind the focused workspace to a Git checkout and sync it. |
| `zerdr unbind` | Remove the focused workspace binding. |
| `zerdr setup` | Install or update the Herdr plugin and global Zed tasks. |
| `zerdr uninstall [--purge]` | Remove integration files. `--purge` also removes zerdr state. |
| `zerdr doctor` | Check capabilities, installed files, bindings, route state, and leases. |

Launch options apply only to bare `zerdr`. Manual commands use the routing mode owned by the live wrapper, regardless of which local terminal runs them.

## Routing behavior

### Internal mode

Start zerdr in a Zed terminal whose current directory belongs to a Git project in that Zed window. zerdr cannot verify the project-to-window relationship through Zed's public API.

The active project becomes the dynamic anchor after both Zed commands succeed. If you remove the current anchor from Zed, switch to another workspace first or restart zerdr from a remaining project.

The `zerdr: Herdr` Zed task remains available when you need an explicit project root. It runs:

```bash
zerdr --mode internal --anchor "$ZED_WORKTREE_ROOT"
```

### External mode

After route and lease admission, external mode runs one `zed --existing TARGET` request for the initially focused workspace. Each later `workspace.focused` event or `zerdr sync` repeats that direct route.

Zed focuses an existing window that already contains the target project. For an unopened project, Zed may add the checkout to an eligible multi-project window instead of opening a new window. Its public CLI does not offer “reuse if open, otherwise force a new window.”

On macOS, external mode defaults to `--focus terminal`. zerdr records the frontmost application, lets Zed route the project, then asks macOS to reactivate the recorded application if Zed is still frontmost. Restoration can fail, flash, move between Spaces, or lose a race with a user switch. It does not require Accessibility or Automation permission.

Use `--focus zed` to leave Zed foreground. Linux uses that policy by default and does not support terminal focus restoration.

## Zed tasks

`zerdr setup` installs these optional labels and prints keybinding examples without editing your keymap:

- `zerdr: Herdr`
- `zerdr: Pick Workspace`
- `zerdr: Next Workspace`
- `zerdr: Previous Workspace`
- `zerdr: Sync Workspace`

zerdr permits one live wrapper across both routing modes. A second launcher exits without replacing the owner route, lease, or Herdr UI. Stop the live wrapper before changing modes.

## Requirements

- **Zed 1.15.0 or newer:** the CLI must expose `--existing` and `--add`.
- **Herdr 0.8.0 or newer:** plugin events must expose `workspace.focused` with protocol 19-compatible responses.
- **Local Git checkouts:** each workspace maps to one canonical checkout root. Linked worktrees remain distinct by checkout path.
- **Local macOS or Linux terminal:** SSH, WSL, containers, and dev containers are rejected. `zerdr doctor`, `--help`, and `--version` remain available remotely; remote doctor runs read-only checks.

Run `zerdr doctor` after setup to inspect the installed command, plugin, tasks, active mode, focus policy, and stale state.

## Notes

- zerdr keeps the fixed Herdr session named `zerdr` after its wrapper exits. Synchronization stops when the wrapper lease ends.
- Each external focus event runs a new Zed request. Use `zerdr sync` when Herdr does not emit another event for an already-focused workspace.
- zerdr does not synchronize Zed project order, remove projects when Herdr workspaces close, or control project-panel folding.
- A missing checkout keeps its stored binding. Restore it, run `zerdr bind PATH`, or remove it with `zerdr unbind`.
- `zerdr uninstall` keeps workspace bindings and route state. Close the live wrapper before `zerdr uninstall --purge`.

## License

MIT
