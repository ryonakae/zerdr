# Automatic Thread Detach on Foreign Pane Focus Implementation Plan

## Requirements

### Problem

A Zed terminal thread holds its Herdr pane through a direct attach client, which pins the pane's PTY size until the client disconnects (Herdr's `direct_attach_resize_locks`; still true on Herdr master after the 0.9.0 "last client to interact controls the tab size" rule, which excludes direct attach clients). A phone-sized Herdr client therefore sees the pane clipped to the thread's grid. v0.7.0 answered this with manual `zerdr detach` / `zerdr attach`, two Zed tasks, and a global flag. The user must remember to run them from the phone and again at the desk.

### Target behaviour

- **Detach when another client selects the pane.** When Herdr emits `pane.focused` for the pane a thread is attached to, the thread's `zerdr connect` terminates its attach child (SIGTERM, as today) and waits, lease held. Herdr then sizes the pane for the client that selected it.
- **Reattach from Zed alone.** While detached, `zerdr connect` owns the thread's tty. It enables focus reporting (DECSET 1004) and SGR mouse reporting (DECSET 1000 + 1006), puts the tty in raw mode, and reattaches on the first input: a focus-in (`ESC [ I` — Zed sends it when the terminal view gains focus, including when a thread is selected in the sidebar and when the Zed window is activated), a mouse event, or any key. Focus-out (`ESC [ O`) is ignored. Input read while detached is discarded, never forwarded. Another client leaving the pane does not reattach.
- **Self-triggered events are ignored.** `connect` focuses the workspace right after attaching, which makes Herdr emit `pane.focused` for that workspace's focused pane. A detach request that arrives within a short grace window after connect's own focus call is dropped.
- **Already-focused panes are accepted as a gap.** Herdr emits `pane.focused` only on focus changes, so a client that connects while the pane is already focused triggers nothing; the user selects another pane and returns. No timer, no fallback.
- **Detached presentation is unchanged.** One notice line in the thread; the sidebar title keeps following the agent with the `[herdr⏸]` marker and the settle bell stays muted (existing `Monitor` behaviour keyed on the `detached` flag).
- **No toggle.** The behaviour is always on. No new settings, flags, or doctor state.
- **Remove the manual mechanism.** `zerdr detach`, `zerdr attach`, `src/suspend.rs`, the global detach flag file, the `.detached` lease markers and `scan_all`, the remote-environment exemption for those commands, the `zerdr: Detach` / `zerdr: Attach` Zed tasks, and their keybinding examples are deleted. `zerdr setup install` removes the two tasks a previous install wrote (byte-owned copies only, as `merge_tasks` already does for no-longer-generated labels).
- **Herdr requirement unchanged.** `pane.focused` exists in Herdr 0.8.0; `min_herdr_version` stays `0.8.0`.

### Out of scope

- Rendering the pane while detached (`herdr terminal session observe`).
- Idle-timer detach, detection of other clients typing into an already-focused pane, and any Herdr-side change.
- Detaching threads whose stdin is not a terminal: such a thread can only be woken by a signal or by its pane disappearing (documented in code, not a user-facing feature).

## Implementation Decisions

- **Event path is the existing plugin hook, not a socket subscription.** The manifest gains `[[events]] on = "pane.focused"` invoking a new hidden command `zerdr detach-from-herdr` (sibling of `sync-from-herdr` / `open-from-herdr`). Herdr runs it with `HERDR_SOCKET_PATH`, `HERDR_PLUGIN_EVENT=pane.focused`, and `HERDR_PLUGIN_CONTEXT_JSON` carrying `workspace_id` and `focused_pane_id` (Herdr sets `HERDR_PANE_ID` too; the JSON is the single source read). The hook does not need the session name: it scans every thread-lease scope for a live record whose `socket_path` equals the canonicalised `HERDR_SOCKET_PATH` and whose `pane_id` matches, then writes a request marker next to that lease. Stale (lockable) records met on the way are removed, as `leased_panes` does. No live lease → exit 0 silently; a plugin hook must never surface an error for panes zerdr does not own.
- **Request marker replaces the old confirmation marker.** `<lease>.detach-request` is written only by the hook and consumed only by the lease-holding connect (existence-based, like the old `.detached` sidecar, so no torn reads). The old `.detached` marker, `thread_detach_active/set/clear`, `Paths::thread_detach_flag_file`, `ThreadLeaseScan`, and `scan_all` go away with `suspend.rs`. Orphan `.detach-request` files whose lease is gone are removed by the hook's scan.
- **`attach_cycle` keeps its two-state shape.** Attached branch: the existing 50 ms cycle poll (`ZERDR_THREAD_CYCLE_POLL_MS`) also checks for the request marker; on a request it deletes the marker, ignores it if inside the grace window, otherwise terminates the child gracefully, prints the notice, and enters the detached branch. Detached branch: raw mode + DECSET, then a `poll(2)` loop over stdin plus the existing `Signals` pending check at the same interval; requests arriving while detached are deleted and ignored. Before spawning the attach child again: write DECRST for 1006/1000/1004, restore the saved termios, then resolve the pane's terminal id and spawn `terminal attach` exactly as today (pane gone → existing graceful exit).
- **Input classification is byte-level and permissive.** Each read is scanned; every `ESC [ O` occurrence is stripped; anything left wakes. Chunks are not reassembled across reads: a focus-out split across two reads is a theoretical case Zed does not produce (it writes the sequence in one call).
- **Grace window** is a constant (2 s) with an environment override `ZERDR_THREAD_FOCUS_GRACE_MS`, following the `ZERDR_THREAD_POLL_MS` precedent so tests can shrink it. Measured from connect's `workspace focus` call; when connect skips the focus (workspace already focused, or list unreadable) there is no window.
- **Terminal control uses `nix`** (`term` for termios, `poll` for stdin readiness), already a dependency. Tests add `nix` with the `term` feature as a dev-dependency to open a pseudo-terminal (in nix 0.31 the `pty` module lives under `term`). No new crates.
- **Connect no longer starts detached.** `start_detached`, `focus_pending`, and the `[herdr⏸]`-at-start path are removed; every connect attaches immediately and focuses the workspace as before.
- **Notice wording** (English, public repo): `zerdr: detached from Herdr because another client selected this pane; focus, click, or press a key here to reattach`.
- **Version 0.8.0** (breaking CLI removal), CHANGELOG under `Changed` (breaking) and `Removed`.

## Contracts

- Hidden CLI: `zerdr detach-from-herdr`. Rejects `--session` like the other hidden plugin commands. Requires `HERDR_PLUGIN_EVENT=pane.focused`, `HERDR_SOCKET_PATH`, and `HERDR_PLUGIN_CONTEXT_JSON` with a string `focused_pane_id`; a missing or malformed environment is an error (mirrors `sync-from-herdr`). Exit 0 whether or not a lease matched.
- Manifest (`assets/herdr/herdr-plugin.toml.in`): two `[[events]]` entries, `workspace.focused → sync-from-herdr` and `pane.focused → detach-from-herdr`. `setup::plugin_is_compatible`, `setup::plugin_has_complete_action`, and `doctor::inspect_manifest` require each exactly once with the exact command, so an outdated manifest fails doctor with the existing "run `zerdr setup install`" guidance.
- State: `ThreadLeaseSet::request_detach(socket_path, pane_id) -> Result<bool>` (true when a live lease was marked) and `ThreadLeaseGuard::take_detach_request() -> Result<bool>` (removes the marker, reports whether it existed). Marker path is the lease path with extension `detach-request`.
- Tty sequences while detached: on entry `ESC[?1004h ESC[?1000h ESC[?1006h`; on exit (reattach, signal, pane gone) `ESC[?1006l ESC[?1000l ESC[?1004l` and termios restored. Wake = any stdin bytes other than `ESC[O`.
- Zed tasks: `zerdr: Herdr` is the only owned label. `assets/zed/keymap.example.json` keeps only the `terminal::SendText` binding.
- Removed public surface: `zerdr detach`, `zerdr attach`; `--help` and bare usage no longer list them; under remote markers they fail as unknown subcommands like any other.

## Tasks

Review base commit: `3fde0a5` (main, in sync with `origin/main` at start).

- [x] Lease request marker (`f52c174`; the removals landed with the connect cycle commit so every commit builds): change `src/state.rs` to drop the detach flag (`thread_detach_active/set/clear`, `Paths::thread_detach_flag_file`), the `.detached` marker (`mark_detached/clear_detached`), `ThreadLeaseScan`, and `scan_all`, and add `ThreadLeaseSet::request_detach` plus `ThreadLeaseGuard::take_detach_request`. Update `tests/state_and_bindings.rs`: remove the flag/marker/scan tests; add a test that `request_detach` marks only a live lease for the matching socket and pane, returns false otherwise, and that `take_detach_request` consumes the marker once.
  - Match `socket_path` after canonicalising both sides (`canonical_socket`); lease records store the canonical path.
- [x] Hook command (`62b94a6`): add `Command::DetachFromHerdr` (hidden) in `src/cli.rs`, dispatch it in `src/lib.rs`, and implement it (in `src/thread.rs` or a small sibling module) reading the plugin environment and calling `request_detach`. Extend `tests/cli_contract.rs`: help/usage tests stop expecting `detach`/`attach`; the hidden-command `--session` rejection test covers `detach-from-herdr`; delete the two detach/attach tests and the remote exemption in `lib.rs`.
- [x] Connect cycle: rewrite `attach_cycle` in `src/thread.rs` per the decisions (request marker → grace check → graceful terminate → detached wait with raw mode, DECSET/DECRST, stdin poll, signals → reattach), remove `start_detached`/`focus_pending`, record the focus-call instant in `run_with_mode`, and update the notice and the module docs. Delete `src/suspend.rs` and its `pub mod`.
  - `SignalForwarder` exists only while a child runs; the detached branch keeps its own `Signals` as today. Termios and DECRST restoration must run on every exit from the detached branch, including `Interrupted` and `PaneGone`.
  - If stdin is not a terminal (`std::io::IsTerminal`) skip termios and DECSET/DECRST and do not poll stdin; the wait then ends only by signal or pane loss.
- [x] Test rig for a real tty: add `nix = { version = "0.31.3", features = ["term"] }` to `[dev-dependencies]` in `Cargo.toml` and a helper in `tests/support/mod.rs` that spawns a `std::process::Command` on a fresh pty (child stdin/stdout/stderr on the slave, master returned for writing input and collecting output in a background reader). Add a `Fixture` helper in `tests/thread_flow.rs` that invokes `detach-from-herdr` with the plugin environment for a pane.
- [x] Thread flow tests (`cargo test --test thread_flow`: 57 passed; `cli_contract` 22, `state_and_bindings` 22): in `tests/thread_flow.rs` replace the flag-driven detach tests (`count_detach_markers`, `the_detach_flag_suspends…`, `a_thread_started_during_detach…`, `zerdr_detach_*`, `zerdr_attach_*`, `a_second_detach_*`) with pty-driven ones, and adapt `a_detached_thread_keeps_a_marked_title_and_never_rings_the_bell`, `reattaching_to_a_missing_pane_ends_the_thread_gracefully`, and `a_signal_during_the_detached_wait_releases_the_lease_and_marker` to the hook-driven detach:
  - a `pane.focused` request for the attached pane terminates the attach child, prints the notice, emits the DECSET sequence, and a focus-in written to the master reattaches through `pane get` + `terminal attach`, with DECRST emitted before the attach;
  - focus-out alone does not reattach; a following key does; an SGR mouse click (`ESC[<0;5;5M`) does;
  - a request inside the grace window after connect's own `workspace focus` is dropped (workspace initially unfocused, `ZERDR_THREAD_FOCUS_GRACE_MS` shrunk so a second request after the window detaches);
  - a request for another pane, and a hook run with no live lease, leave the thread attached and exit 0 with nothing written;
  - a thread with non-tty stdin stays detached (still alive, lease held) after a request instead of spinning or exiting.
- [x] Setup, manifest, doctor (`cargo test --test setup_and_doctor`: 54 passed; `herdr_wrapper`: 20 passed — the launcher now also requires the `pane.focused` hook and sends an older install to `zerdr setup install`, covered by the rewritten pre-action test): update `assets/herdr/herdr-plugin.toml.in`, `assets/zed/tasks.json.in`, `assets/zed/keymap.example.json`, `OWNED_LABELS` and the completion message in `src/setup.rs`, the manifest checks in `src/setup.rs` and `src/doctor.rs`. Update `tests/setup_and_doctor.rs`: install writes both events and only the `zerdr: Herdr` task; an install over a v0.7.0 state (`zerdr: Detach`/`zerdr: Attach` fingerprinted in `InstallState`) removes those tasks and keeps a user-modified copy; doctor fails a manifest that lacks the `pane.focused` event.
- [x] Docs and release: rewrite the "Sharing the session with a small client" section of `README.md` (automatic detach on foreign focus, reattach by focusing/clicking/typing in the thread, the already-focused gap and its workaround, Shift+drag note unchanged), update the commands table, the `setup install` row, and the Keybindings note; update the repository map and `thread.rs`/`suspend.rs` lines in `AGENTS.md`; add the v0.8.0 CHANGELOG entry and bump `Cargo.toml`/`Cargo.lock` to 0.8.0.

## Final Validation

- [ ] Detach/reattach behaviour, grace window, ignored inputs, non-tty wait: `cargo test --test thread_flow` → all pass, including on Ubuntu CI (pty and termios paths are Unix-generic).
- [ ] Lease marker contract: `cargo test --test state_and_bindings` → pass.
- [ ] CLI surface: `cargo test --test cli_contract` → pass; `cargo run --locked -- --help` lists `connect`, `start`, `workspace`, `setup` and neither `detach` nor `attach` nor `detach-from-herdr`.
- [ ] Manifest, tasks, doctor: `cargo test --test setup_and_doctor` → pass.
- [ ] Sync hook unaffected: `cargo test --test sync_flow` → pass (`sync-from-herdr` still rejects events other than `workspace.focused`).
- [ ] Removal is complete: `grep -rn "suspend\|thread_detach\|\.detached\|zerdr: Detach\|zerdr: Attach\|zerdr detach\|zerdr attach" src tests assets README.md AGENTS.md` → only the CHANGELOG history, this plan, and the `setup_and_doctor` upgrade test that seeds the two legacy task labels mention them.
- [ ] Required CI checks in order: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-targets --all-features` → all pass.
- [ ] Manual, against the developer's real environment only after the automated checks: `zerdr setup install`, open a terminal thread with `zerdr connect`, select the pane from a second Herdr client (phone over SSH or `herdr` in another terminal) → the thread prints the notice and the pane fits the second client; click the thread or select it in the sidebar → the thread reattaches and the pane returns to the thread's size.

> 各タスクは対応する検証が成功してから完了にする。実装中の軽微な差分と検証結果は該当箇所へ反映し、要件、対象外、公開契約の変更はユーザーへ確認する。最終確認では有効な検証結果を再利用し、計画と実際の変更が一致することを確認する。必要な検証と実装側の必須レビューが通ったら、計画を同名のまま `docs/plans/archived/` へ移す。
