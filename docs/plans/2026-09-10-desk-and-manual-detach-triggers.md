# Desk and Manual Detach Triggers Implementation Plan

## Requirements

### Problem

Since v0.8.0 (unreleased, `f51692f`) a thread detaches when Herdr emits `pane.focused` for its pane. Manual verification showed the two everyday flows never produce that event:

- On the desk, the user keeps Zed and a ghostty `herdr` side by side. Switching to ghostty and typing into the pane it already shows changes no focus, so Herdr stays silent and the pane stays pinned to the thread's size.
- On the phone, `herdr` opens on the pane it last showed; same silence.

Herdr exposes no client-focus or client-input signal to plugins or the CLI, and direct attach clients are excluded from Herdr's own multi-client sizing, so zerdr has to release the attach from signals it can observe: Zed itself, and an explicit request from the other client.

### Target behaviour

- **Desk: detach when Zed goes to the background (macOS).** While attached, `zerdr connect` running inside a Zed terminal watches which application is frontmost. When Zed has not been frontmost for a grace period (default 2 s) the thread detaches exactly as on a `pane.focused` request. Nothing reattaches on its own; the existing Zed-side wake (focus-in on window activation, click, key) does. Zed staying frontmost while the user works in the editor never detaches. Outside a Zed terminal, or where the frontmost application cannot be read (Linux), the trigger is inactive.
- **Other client: one key.** A Herdr plugin action, bindable in Herdr's keymap, asks the Zed thread attached to the pane the invoking client currently focuses to detach. With no such thread it reports that through a Herdr notification instead of failing silently.
- **Before opening Herdr: `zerdr detach`.** A public command that asks every live thread (all sessions) to detach, usable from an SSH shell on the same machine before the phone runs `herdr`. It does not wait for confirmation and there is no `zerdr attach`: threads come back through the Zed-side wake.
- **Unchanged:** the `pane.focused` hook, the shared focus grace window, the detached presentation (`[herdr⏸]`, muted bell, one notice line), the wake rules, and the herdr 0.8.0 requirement.

### Out of scope

- Detecting the phone or another client taking focus without user action (needs a Herdr change; upstream request to be filed separately).
- A pty proxy reading Zed's focus-out sequence (rejected: indistinguishable from clicking the Zed editor).
- Linux support for the frontmost check.
- Waiting for detach confirmation or reporting per-thread results from `zerdr detach`.

## Implementation Decisions

- **Frontmost check lives in `src/zed.rs`** as `Zed::frontmost_is_zed() -> Option<bool>`: `None` when unknown (non-macOS, or the seam is absent in tests), `Some(true)` when the frontmost application's bundle identifier starts with `dev.zed.Zed` (covers Preview and Nightly). macOS uses the already-declared `objc2-app-kit` (`NSWorkspace::sharedWorkspace().frontmostApplication()`). Test seam, following `ZERDR_TEST_REMOTE_MARKERS`: with `ZERDR_TEST_ROOT` set, `ZERDR_TEST_ZED_FRONTMOST_FILE` names a file whose content `1`/`0` is the answer (missing file → `None`).
- **Trigger gating.** The attached loop consults the frontmost check only when `TERM_PROGRAM=zed` (Zed's integrated terminal sets it; verified on Zed 1.18.1) and the check returns `Some`. It polls at most every 500 ms (separate from the 50 ms cycle poll), keeps `background_since: Option<Instant>`, and detaches when the background run reaches the grace. Grace: constant 2 s, override `ZERDR_THREAD_BACKGROUND_GRACE_MS`. The detach path is the existing one (terminate child, notice, detached wait); the notice text stays the same.
- **The plugin action reuses `detach-from-herdr`.** Manifest gains `[[actions]] id = "detach-thread" title = "Release Zed thread" contexts = ["pane"] command = [exe, "detach-from-herdr"]`. The entry point accepts either `HERDR_PLUGIN_EVENT=pane.focused` or `HERDR_PLUGIN_ACTION_ID=detach-thread`; anything else stays an error. Both read `focused_pane_id` from the context. Event path: silent when no lease matches (as now). Action path: when no lease matches, call `herdr notification show` through the existing adapter (`Herdr::notify_error_for`, best-effort) with `no Zed thread is attached to pane <id>`, exit 0. The action ignores the focus grace window (an explicit key is never an echo) — implemented by writing the request marker as today; the consuming connect cannot tell the source, so the action marks the request as explicit by writing `explicit` into the marker file and `take_detach_request` reports it, letting the cycle skip the grace check for explicit requests.
- **Setup and doctor verify both actions and both events.** Generalise the existing single-action checks into a table `ACTIONS: [(id, title, contexts, subcommand)]` next to `EVENT_HOOKS`, used by `plugin_has_complete_action`, `inspect_manifest`, and the `compatible_plugins_json` test fixture. `plugin_is_compatible` (launcher) keeps checking events only. `assets/herdr/keymap.example.toml` adds a `prefix+shift+d` → `zerdr.detach-thread` binding; setup prints it as it prints the Open Zed one.
- **`zerdr detach`** is `Command::Detach` (public, no arguments, `--session` rejected like before). It calls `ThreadLeaseSet::request_detach_all() -> Result<usize>` — every live lease in every scope gets an explicit marker; stale records and orphan markers are removed as `request_detach` does — and prints `zerdr: asked N thread(s) to detach` or `zerdr: no live threads`. It joins `setup doctor` and the plugin hooks in the remote-environment exemption (it only touches local state files).
- **Version stays 0.8.0**; the CHANGELOG `Changed` bullet is amended (manual `zerdr detach` returns as a fire-and-forget request; `zerdr attach` and the Zed tasks stay removed) and a bullet describes the desk trigger and the action.

## Contracts

- `Zed::frontmost_is_zed() -> Option<bool>`; seam `ZERDR_TEST_ZED_FRONTMOST_FILE` (test builds only in effect: gated by `ZERDR_TEST_ROOT`).
- Env overrides: `ZERDR_THREAD_BACKGROUND_GRACE_MS` (default 2000).
- Hidden CLI `zerdr detach-from-herdr`: event mode as before; action mode requires `HERDR_PLUGIN_ACTION_ID=detach-thread`, `HERDR_SOCKET_PATH`, context `focused_pane_id`; exit 0 in both modes, notification on a missing lease in action mode only.
- Public CLI `zerdr detach`: exit 0; output as above; rejects `--session`; runs under remote markers.
- State: `ThreadLeaseSet::request_detach_all() -> Result<usize>`; `ThreadLeaseGuard::take_detach_request() -> Result<Option<DetachRequest>>` where `DetachRequest { explicit: bool }` (marker body `explicit` ⇔ true). `request_detach(socket, pane, explicit: bool)`.
- Manifest: two actions (`open-zed`, `detach-thread`) and two events, each exactly once with the exact command; doctor and setup fail otherwise with the existing guidance.

## Tasks

Review base commit: `f51692f`.

- [x] Explicit requests and detach-all (`cargo test --test state_and_bindings`: 24 passed; the attach cycle already honours `explicit` so the API compiles end to end): extend `src/state.rs` (`request_detach(.., explicit)`, `request_detach_all`, `DetachRequest`, `take_detach_request` returning it) and update `tests/state_and_bindings.rs` (explicit flag round-trips; `request_detach_all` marks every live lease across two sessions and cleans stale records).
- [x] `zerdr detach` command (`cli_contract` 25 passed; `thread_flow` two-thread test passed): `src/cli.rs`, `src/lib.rs` (dispatch, remote exemption, `--session` rejection), implementation in `src/thread.rs`. `tests/cli_contract.rs`: help lists `detach`; bare usage lists it; `--session` rejected; runs under remote markers. `tests/thread_flow.rs`: two pty threads detach after one `zerdr detach` (both notices, both attach clients gone), output counts 2; with no threads prints the no-threads line.
- [x] Action mode of the hook (`thread_flow` action tests passed; `cli_contract` 25 passed): `src/thread.rs::detach_from_herdr` accepts `HERDR_PLUGIN_ACTION_ID=detach-thread`, writes an explicit marker, notifies on a missing lease; the attach cycle skips the grace check for explicit requests. `tests/thread_flow.rs`: action detaches inside the grace window; action with no lease calls `notification show` (fake herdr log) and exits 0; wrong action id fails.
- [ ] Desk trigger: `src/zed.rs::frontmost_is_zed` with the macOS implementation and the test seam; `src/thread.rs` attached loop polls it under `TERM_PROGRAM=zed`, with `background_since` and the grace. `tests/thread_flow.rs` (pty, seam file, `ZERDR_THREAD_BACKGROUND_GRACE_MS=300`, `TERM_PROGRAM=zed` set explicitly): background beyond the grace detaches and a later focus-in reattaches; background shorter than the grace keeps the attach; without `TERM_PROGRAM=zed` the background never detaches; seam file absent never detaches.
  - Tests inherit the developer shell's `TERM_PROGRAM=zed`; set or remove it explicitly in every connect spawned by these tests.
- [ ] Manifest, setup, doctor: `assets/herdr/herdr-plugin.toml.in` (action), `assets/herdr/keymap.example.toml` (binding), `src/setup.rs` and `src/doctor.rs` (`ACTIONS` table), `tests/support/mod.rs` (`compatible_plugins_json` gains the action), `tests/setup_and_doctor.rs` (install writes both actions; doctor fails a manifest lacking `detach-thread`; setup output prints the new binding).
- [ ] Docs: `README.md` (sharing section: the three triggers, the `prefix+shift+d` example, `zerdr detach`; commands table row; Requirements note that the desk trigger is macOS-only), `CHANGELOG.md` v0.8.0, `AGENTS.md` (thread.rs and zed.rs lines; `detach` added to the remote exemption note).

## Final Validation

- [ ] Desk trigger, action mode, `zerdr detach`, explicit-request grace bypass: `cargo test --test thread_flow` → pass.
- [ ] State contract: `cargo test --test state_and_bindings` → pass.
- [ ] CLI surface: `cargo test --test cli_contract` → pass; `cargo run --locked -- --help` lists `detach`.
- [ ] Manifest and doctor: `cargo test --test setup_and_doctor` and `cargo test --test herdr_wrapper` → pass.
- [ ] Required CI checks in order: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-targets --all-features` → pass (Ubuntu CI exercises the non-macOS `None` path).
- [ ] Manual (user): rebuild, `./target/release/zerdr setup install`, connect a thread; (1) switch to ghostty for 3 s → thread detaches, ghostty sizes the pane; click the thread → reattaches. (2) In ghostty's herdr press `prefix+shift+d` on the pane → detaches. (3) Over SSH run `zerdr detach` then `herdr` → pane at phone size; back at Zed, click → reattaches. If the `pane` action context does not make the key binding fire, fall back to `contexts = ["workspace"]` (Herdr's context semantics are not documented).

> 各タスクは対応する検証が成功してから完了にする。実装中の軽微な差分と検証結果は該当箇所へ反映し、要件、対象外、公開契約の変更はユーザーへ確認する。最終確認では有効な検証結果を再利用し、計画と実際の変更が一致することを確認する。必要な検証と実装側の必須レビューが通ったら、計画を同名のまま `docs/plans/archived/` へ移す。
