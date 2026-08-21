# Thread Auto Mode and CLI Cleanup Implementation Plan

> **For implementers:** Execute tasks in order unless dependencies allow otherwise. Mark a task complete only after its validation succeeds. Reflect minor implementation differences in the relevant task. Ask the user before changing requirements, Out of Scope, or public contracts.

## Problem Statement

Two things came out of real-world use of `zerdr thread`:

1. Typing `zerdr thread` in every new Zed terminal thread is tedious. The user wants an
   explicit auto mode: when enabled, every new terminal thread attaches to Herdr
   automatically; when disabled, nothing happens (today's behavior).
2. Parts of the original CLI turned out to be unused. External routing (opening Zed from a
   Ghostty-hosted Herdr) steals window focus and was judged "not useful in practice".
   `pick`/`next`/`previous` duplicate switching that the user always does in the Herdr TUI.

## Goal

- `zerdr thread --enable` / `--disable` toggles a global auto mode. While enabled, Zed's
  `agent.terminal_init_command` (installed once by `--enable`) runs `zerdr thread --auto`,
  which attaches new terminal threads best-effort; while disabled, `--auto` is a silent no-op.
- The external routing mode, its flags, and the `pick`/`next`/`previous` subcommands are
  removed. Generated Zed tasks shrink to the single "zerdr: Herdr" task, and `zerdr setup`
  cleans up the previously installed owned tasks.

## Out of Scope

- Per-project auto-mode scoping (global only; noted as a future extension).
- Distinguishing restored terminal threads from new ones on Zed restart (no signal exists;
  reattach-on-restart is accepted as resume behavior).
- Automatic Herdr workspace creation from `--auto` (manual `zerdr thread --create` only).
- Renaming or regrouping the remaining commands (bare `zerdr` keeps its `herdr` symmetry).
- Detecting external attach clients in leases (pre-existing known limitation).

## Requirements and Decisions

### Requirements

Cleanup:

- **R1:** Remove external routing entirely: the `--mode` and `--focus` root flags,
  `LaunchMode`/`FocusPolicy`, `src/focus.rs`, `RouteFocus`, and `RouteStrategy::External`.
  Bare `zerdr` always routes internally; `--session` and `--anchor` keep working. Launch
  location is not validated (running outside a Zed terminal is allowed and documented).
- **R2:** Remove the `pick`, `next`, and `previous` subcommands and `src/picker.rs`.
  `sync` remains as the manual recovery command. `bind`/`unbind` and the hidden
  `sync-from-herdr`/`open-from-herdr` commands are unchanged.
- **R3:** `generated_tasks` produces only the "zerdr: Herdr" task. `zerdr setup` removes a
  previously installed task whose label is recorded in the prior install state and whose
  fingerprint still matches, even though the label is no longer generated. A modified or
  foreign task with an old owned label is preserved and reported, mirroring uninstall's
  `preserving modified or foreign Zed task` warning.
- **R4:** A stale route-state file recorded with the removed external mode fails to
  deserialize and is handled by the existing corrupt-route reporting; no migration or
  silent conversion is added.

Auto mode:

- **R5:** Auto mode is a single global flag: a file under zerdr's state directory whose
  presence means enabled. No content is interpreted.
- **R6:** `zerdr thread --enable` requires an existing install state (error directing to
  `zerdr setup` otherwise, matching the existing `; run \`zerdr setup\`` guidance style).
  It then: installs `agent.terminal_init_command = "<executable> thread --auto"` into Zed
  settings when the key is absent (settings backup, symlink write-through, CST edit, and
  fingerprint recording via the existing setup machinery); errors without writing when a
  different value is present; treats an already-owned matching value as success. Finally it
  creates the flag file. The whole command is idempotent.
- **R7:** `zerdr thread --disable` removes the flag file only. Zed settings are untouched.
  Idempotent.
- **R8:** `zerdr thread --auto` with auto mode disabled exits 0 with no output and without
  invoking Herdr. With auto mode enabled it behaves exactly like manual `zerdr thread`
  (attach a free agent, else open a fresh Herdr tab with a plain shell), except that any
  failure is reported as a single `zerdr: ...` line on stderr followed by exit 0, leaving
  the thread usable as a plain local shell. `--auto` never creates a Herdr workspace.
- **R9:** `--enable`, `--disable`, and `--auto` are mutually exclusive and conflict with
  `TARGET`, `--kind`, and `--create`. `--enable`/`--disable` also conflict with
  `--session`; `--auto` accepts `--session` like manual attach. `ZERDR_THREAD_KIND`
  continues to apply to the shared attach flow, including `--auto`.
- **R10:** `zerdr setup` stops migrating away an owned init command and preserves
  `terminal_init_command_fingerprint` from the previous install state when rewriting
  install.json. (Today setup hardcodes `None`, which would erase the ownership recorded by
  `--enable` and orphan the settings value — the historical lost-fingerprint bug.)
- **R11:** `zerdr uninstall` keeps removing the owned init command (existing fingerprint
  path) and additionally deletes the auto-mode flag file.
- **R12:** `zerdr doctor` reports auto mode informationally (never failing): enabled with
  the init command installed, enabled but the init command missing or foreign (warn),
  or disabled. The existing `report_init_command` text is folded into this.
- **R13:** README reflects both changes: internal-only routing, the reduced command table,
  and a terminal-threads section describing `--enable`/`--disable`/`--auto`, including
  reattach-on-restart as accepted resume behavior.

### Implementation Decisions

- **D1:** State-flag approach over settings rewriting: the init command stays installed
  once; toggling touches only zerdr state. Chosen because the user's Zed settings are a
  dotfiles-managed symlink and per-toggle diffs are noise (dig Q1).
- **D2:** `--enable` owns the settings write, not `setup`; setup keeps its
  "never touches settings" contract from the opt-in change (dig Q2).
- **D3:** Best-effort `--auto`: one-line note + exit 0 on failure, because the init command
  runs unconditionally in every new thread including projects without Herdr (dig Q4).
- **D4:** Cleanup lands before the auto-mode feature so the new code targets the reduced
  surface (fewer flags, one Zed task) and docs/tests are written once.
- **D5:** No heuristic to suppress reattach on Zed restart; leases release on exit so
  reattach is resume, and turning the mode off is the escape hatch (dig Q5).

### Contracts

CLI surface after both changes:

```text
zerdr [--session NAME] [--anchor PATH]          # wrapper, internal routing only
zerdr [--session NAME] sync
zerdr [--session NAME] bind [PATH] | unbind
zerdr [--session NAME] thread [TARGET] [--kind KIND] [--create]
zerdr thread --enable | --disable
zerdr [--session NAME] thread --auto
zerdr setup | uninstall [--purge] | [--session NAME] doctor
```

- Installed init command value: `"<stable executable path> thread --auto"` (produced by
  `terminal_init_command`, which changes from `"<exe> thread"`).
- Auto-mode flag: file presence under `Paths` state directory; exact filename is internal
  but must be covered by `uninstall` (removed) and `--purge` (removed with state dir).
- `InstallState.terminal_init_command_fingerprint: Some(_)` means zerdr owns the current
  settings value; `--enable` records it, setup preserves it, uninstall consumes it.
- Generated Zed tasks: exactly one task labeled "zerdr: Herdr".

## Current Context

### Confirmed

- `merge_tasks` (src/setup.rs:376) skips existing tasks whose label is not in the generated
  list, so shrinking the task list leaves the four old owned tasks behind without new
  cleanup logic. `remove_owned_tasks` (uninstall) already removes by recorded fingerprints.
- `setup` (src/setup.rs:230) rewrites install.json with
  `terminal_init_command_fingerprint: None` and migrates away an owned init command
  (src/setup.rs:219-222); both must change for R10.
- Settings-write machinery exists and is reusable: `resolve_config_file` (symlink
  write-through), `backup_before_mutation`, `write_checked`, `installed_init_command`,
  `remove_owned_init_command`, JSONC CST editing.
- External routing spans: `--mode`/`--focus` in src/cli.rs, `resolve_launch` +
  `in_zed_terminal` in src/runtime.rs, `RouteFocus`/`RouteStrategy::External` in
  src/state.rs, `with_external_focus` in src/focus.rs, two call sites in src/sync.rs
  (lines 207, 495), route reporting in src/doctor.rs (imports at line 17; the
  `route mode: external` / `focus policy` branches around lines 193–223), and tests in
  tests/cli_contract.rs and tests/sync_flow.rs.
- The generated "zerdr: Herdr" task passes `["--mode", "internal", "--anchor",
  "$ZED_WORKTREE_ROOT"]` (assets/zed/tasks.json.in:5), and
  tests/setup_and_doctor.rs:92 asserts those args literally; removing `--mode` breaks
  both unless the task definition changes with it.
- src/doctor.rs hardcodes the five-task count: "all five owned Zed task payloads are
  valid" (lines 113, 296) and an "exactly five tasks" message gated by
  `owned_labels().len()` (line 398).
- assets/zed/keymap.example.json binds ctrl-alt-p/j/k/s to `task::Spawn` for the four
  workspace tasks being removed, plus the `terminal::SendText` binding that stays.
- `pick`/`next`/`previous` dispatch through `run_manual` to `Synchronizer::pick`/`navigate`
  (src/lib.rs:82-90); `picker::choose` is used only by `Synchronizer::pick`.
- Owned task labels today: "zerdr: Herdr", "zerdr: Pick Workspace", "zerdr: Next
  Workspace", "zerdr: Previous Workspace", "zerdr: Sync Workspace" (src/setup.rs:18-22).
- `thread::run(session, target, kind, create)` is dispatched from src/lib.rs:100-110;
  attach failures currently surface as fatal `Error::User` values.
- Test seams: integration tests run the real binary against fake `herdr`/`zed` scripts via
  `TestEnv` (tests/support/mod.rs); `ZERDR_TEST_ROOT` redirects `Paths`. AGENTS.md forbids
  running setup/uninstall/doctor against the real environment and requires the shared fake
  herdr to stay free of per-invocation setup.

### Assumptions

- The auto-mode flag filename (e.g. `thread-auto`) and whether a `Paths` field is added are
  implementation details; any name under the state dir satisfies R5/R11.
- Where the enable/disable logic lives (thread.rs vs a helper in setup.rs) is free as long
  as the existing settings machinery is reused rather than duplicated.

## File Structure

- Modify: `src/cli.rs` — drop `--mode`/`--focus`, `LaunchMode`, `FocusPolicy`,
  `Pick`/`Next`/`Previous`; add `--auto`/`--enable`/`--disable` to `Thread` with conflicts.
- Modify: `src/lib.rs` — dispatch changes for removed and new commands; drop `focus`/`picker` modules.
- Modify: `src/runtime.rs` — `resolve_launch` becomes internal-only (keep `--anchor` and
  remote rejection); remove `in_zed_terminal` if unused.
- Modify: `src/state.rs` — remove `RouteFocus` and `RouteStrategy::External`; add the
  auto-mode flag path/helpers.
- Delete: `src/focus.rs`, `src/picker.rs`.
- Modify: `src/sync.rs` — remove `pick`, `navigate`, picker import, external match arms.
- Modify: `src/setup.rs` — single generated task; stale owned-task cleanup in
  `merge_tasks`; preserve init-command fingerprint; drop the setup-time migration; updated
  hint text (`zerdr thread --enable`); `terminal_init_command` returns `"<exe> thread --auto"`;
  uninstall deletes the flag file; enable/disable settings write helper.
- Modify: `src/thread.rs` — enable/disable/auto entry points; best-effort wrapper for auto.
- Modify: `src/doctor.rs` — drop external-route reporting; fix hardcoded five-task
  wording; auto-mode report replacing/absorbing `report_init_command`.
- Modify: `assets/zed/tasks.json.in` — "zerdr: Herdr" task args drop `--mode internal`.
- Modify: `assets/zed/keymap.example.json` — drop the four `task::Spawn` bindings for
  removed tasks; keep the `terminal::SendText` binding.
- Test: `tests/cli_contract.rs`, `tests/sync_flow.rs`, `tests/herdr_wrapper.rs`,
  `tests/setup_and_doctor.rs`, `tests/thread_flow.rs`, `tests/support/mod.rs` (only if a
  fake needs a new hook; keep the shared fake herdr byte-stable).
- Modify: `README.md` — routing section, command table, terminal-threads section, notes.

## Testing Decisions

- **Test seam:** the compiled `zerdr` binary run by integration tests against fake
  `herdr`/`zed` scripts and `ZERDR_TEST_ROOT`-redirected state, same as all existing tests.
- **Behavior:** CLI acceptance/rejection via clap exit codes and stderr; settings/tasks
  file contents after setup/enable/uninstall; `--auto` exit codes, stdout/stderr, and
  which fake-herdr calls were recorded.
- **Prior art:** `tests/setup_and_doctor.rs` (`seed_owned_install`, symlink write-through
  tests), `tests/thread_flow.rs` (attach fixtures), `tests/cli_contract.rs` (flag conflicts).
- **Avoid:** unit-testing private setup functions; asserting on JSONC formatting beyond the
  edited keys; depending on fake-herdr timing (keep the shared fake unchanged).

## Progress

- [x] Task 1: Remove external routing
- [x] Task 2: Remove pick/next/previous and shrink Zed tasks
- [x] Task 3: Auto-mode flag with `--enable`/`--disable`
- [x] Task 4: `--auto` best-effort attach
- [x] Task 5: Documentation

## Tasks

### Task 1: Remove external routing

**Covers:** R1, R4, D4

**Objective:** `zerdr` no longer knows about external routing: `--mode` and `--focus` are
unknown flags, bare `zerdr` always routes internally, and the focus-restore code is gone.

**Files:**
- Modify: `src/cli.rs`, `src/lib.rs`, `src/runtime.rs`, `src/state.rs`, `src/sync.rs`,
  `src/doctor.rs`, `assets/zed/tasks.json.in`
- Delete: `src/focus.rs`
- Test: `tests/cli_contract.rs`, `tests/sync_flow.rs`, `tests/herdr_wrapper.rs`,
  `tests/setup_and_doctor.rs`

**Dependencies:** none

**Implementation notes:**
- `resolve_launch` keeps remote rejection and the `--anchor`/cwd → `canonical_git_root`
  resolution; everything mode/focus-related goes. `RouteStrategy` keeps only `Internal`,
  staying a serde-tagged enum so existing internal route files keep parsing; an old
  external route file then fails to parse and flows through the existing corrupt-route
  handling (R4) — do not add migration.
- The "zerdr: Herdr" task args drop `--mode internal` (assets/zed/tasks.json.in), keeping
  `--anchor $ZED_WORKTREE_ROOT`. The changed fingerprint makes `merge_tasks` replace the
  installed owned task on the next setup, which is the intended upgrade path. Update the
  literal args assertion in tests/setup_and_doctor.rs:92.
- src/doctor.rs: remove the `RouteFocus`/`External` imports and the
  `route mode: external` / `focus policy` report branches; doctor behavior for internal
  routes is unchanged.
- Delete external-mode tests rather than porting them; keep internal wrapper tests green
  without `ZED_TERM` gating (the wrapper no longer cares where it starts).

**Test cases:**
- `zerdr --mode external` and `zerdr --focus zed` → clap unknown-argument error (exit 2).
- `zerdr --help` no longer mentions `--mode` or `--focus`.
- Existing internal wrapper flow (route write, workspace switch → `zed --existing`/`--add`)
  still passes without Zed-terminal environment variables.
- Bare `zerdr --anchor PATH` still resolves the anchor to a canonical git root.

**Complete when:**
- No references to `LaunchMode`, `FocusPolicy`, `RouteFocus`, `External`, or `focus.rs` remain.
- Internal routing tests in `tests/herdr_wrapper.rs` and `tests/sync_flow.rs` pass.
- Validation succeeds.

**Validation:**
- Run: `cargo test --all-targets --all-features`
- Expected: all tests pass; removed-flag tests assert rejection.

**Result:** Done. All planned files changed, plus `AGENTS.md` (repository-map lines
mentioning `focus.rs` and routing mode) and `tests/state_and_bindings.rs` (the
external-route persistence test and now-unused imports were removed there, not only in the
files listed above). Launcher preflight tests that used `--mode external` as a vehicle
were ported to internal anchors via a new `anchor_repo` helper in
`tests/herdr_wrapper.rs`; external-only behavior tests were deleted
(`one_shot_action_with_a_live_external_terminal_route_never_restores_focus`,
`external_terminal_focus_restores_after_zed_success_and_failure`,
`repeated_external_events_each_run_one_existing_call_without_mutating_route`,
`bind_with_live_external_wrapper_persists_then_uses_the_existing_route` — the last is
covered by `manual_bind_normalizes_nested_path_and_synchronizes`). Validation: full test
suite green (179 tests), `cargo fmt --check` clean, clippy `-D warnings` clean.

### Task 2: Remove pick/next/previous and shrink Zed tasks

**Covers:** R2, R3

**Objective:** the manual workspace UI commands are gone, `sync` remains, and setup
installs exactly one Zed task while cleaning up the four stale owned tasks on upgrade.

**Files:**
- Modify: `src/cli.rs`, `src/lib.rs`, `src/sync.rs`, `src/setup.rs`, `src/doctor.rs`
- Delete: `src/picker.rs`
- Test: `tests/cli_contract.rs`, `tests/sync_flow.rs`, `tests/setup_and_doctor.rs`

**Dependencies:** Task 1 (shares `src/cli.rs`/`src/lib.rs`/`src/sync.rs` edits)

**Implementation notes:**
- `merge_tasks` must also consider labels recorded in the previous install state that are
  no longer generated: fingerprint match → remove the element; mismatch → leave it and
  report `preserving modified or foreign Zed task <label>` on stderr (same wording as
  uninstall). The new install state records fingerprints only for generated tasks.
- `owned_labels()` shrinks to the single label. src/doctor.rs hardcodes the old count and
  must follow: "all five owned Zed task payloads are valid" (lines 113, 296) and the
  "exactly five tasks" message near line 398 (already gated by `owned_labels().len()`,
  but the wording embeds "five").

**Test cases:**
- `zerdr pick` → clap unknown-subcommand error; same for `next`/`previous`.
- setup over a tasks file containing all five previously-owned tasks (fingerprints seeded
  in install state) → only "zerdr: Herdr" remains, backup written.
- setup over a tasks file where "zerdr: Pick Workspace" was hand-modified → task preserved,
  warning printed, other stale tasks removed.
- `zerdr sync` still dispatches (existing sync tests keep passing).
- doctor after the new setup reports the single task without failing.

**Complete when:**
- `picker.rs` is gone and `Synchronizer` has no `pick`/`navigate`.
- Upgrade cleanup verified by the seeded-tasks tests.
- Validation succeeds.

**Validation:**
- Run: `cargo test --all-targets --all-features`
- Expected: all tests pass, including the new stale-task cleanup tests.

**Result:** Done. Also changed `assets/zed/tasks.json.in` (the four workspace-task entries
removed; the template feeds `generated_tasks`). Stale cleanup and preserve-modified are
covered by `setup_removes_stale_owned_tasks_recorded_by_an_older_install`; other tests
that used removed commands or tasks as vehicles were retargeted to `sync` / the Herdr
task (`setup_restores_a_missing_owned_task`, `doctor_rejects_a_modified_owned_task_payload`,
`uninstall_preserves_an_owned_task_modified_after_setup`,
`generated_task_command_executes_when_the_binary_path_contains_spaces`);
`invalid_target_preflight_changes_neither_herdr_nor_zed` was deleted because its code path
(`switch_or_sync` target preflight) only existed for pick/next/previous. Validation: full
suite green (175 tests), fmt clean, clippy `-D warnings` clean.

### Task 3: Auto-mode flag with `--enable`/`--disable`

**Covers:** R5, R6, R7, R9 (conflict rules), R10, R11, R12, D1, D2

**Objective:** `zerdr thread --enable` installs the init command once and turns the mode
on; `--disable` turns it off; setup/uninstall/doctor respect the new ownership.

**Files:**
- Modify: `src/cli.rs`, `src/lib.rs`, `src/state.rs`, `src/setup.rs`, `src/thread.rs`, `src/doctor.rs`
- Test: `tests/setup_and_doctor.rs`, `tests/cli_contract.rs`

**Dependencies:** Task 2 (setup.rs churn)

**Implementation notes:**
- `--enable` flow: load install state (error `...; run \`zerdr setup\`` when missing) →
  resolve settings file → read current `agent.terminal_init_command` via
  `installed_init_command` → absent: backup + CST-insert the value + record fingerprint in
  install.json; equal to ours with matching fingerprint: no write; anything else: error
  naming the current value, nothing written, no flag created. Then create the flag file.
  Print one line stating the mode is enabled (and whether settings were written).
- `terminal_init_command` changes to `"<exe> thread --auto"`; update the setup hint text to
  point at `zerdr thread --enable` instead of manual settings editing.
- setup: delete the `remove_owned_init_command` migration block and carry
  `terminal_init_command_fingerprint` forward from the previous install state (R10).
- uninstall: after the existing owned-init-command removal, delete the flag file if present.
- doctor: replace `report_init_command` with an auto-mode report (R12): disabled → pass
  "thread auto mode is disabled; attach manually with `zerdr thread` or run
  `zerdr thread --enable`" (wording free); enabled + owned value installed → pass; enabled
  but value missing/foreign → warn, never fail.
- Executable-path staleness (enable recorded one path, binary reinstalled elsewhere) is
  already handled by fingerprinting the value string; a mismatch surfaces as the doctor
  warn case. No extra handling.

**Test cases:**
- `zerdr thread --enable` without install state → error mentioning `zerdr setup`, no flag,
  settings untouched.
- enable with seeded install state and no init command → settings contain
  `"<exe> thread --auto"`, backup file exists, install.json fingerprint set, flag exists;
  running enable again changes nothing and still exits 0.
- enable when settings already contain a foreign init command → exit 1, settings and
  install.json unchanged, flag absent.
- enable through a symlinked settings file → real file written, symlink intact.
- `zerdr thread --disable` → flag gone, settings still contain the value; second disable
  exits 0.
- setup after enable → init command still present, fingerprint still recorded.
- uninstall after enable → init command removed from settings, flag file removed.
- doctor in enabled and disabled states → informational lines, exit 0 both ways.
- `zerdr thread --enable --disable`, `--enable TARGET`, `--enable --session s` → clap
  conflict errors.

**Complete when:**
- Enable/disable round-trip verified end to end including setup/uninstall interplay.
- No path exists where setup erases a fingerprint recorded by enable.
- Validation succeeds.

**Validation:**
- Run: `cargo test --test setup_and_doctor --all-features && cargo test --all-targets --all-features`
- Expected: all tests pass.

**Result:** Done. Enable/disable live in `src/setup.rs` (`thread_auto_enable`/`thread_auto_disable`/
`thread_auto_enabled`); the flag is `Paths.thread_auto_flag_file` (`<state>/thread-auto`).
Implementation notes beyond the plan: the fingerprint is recorded in install.json *before*
the settings write (a fingerprint without a value is harmless; the reverse orphans the
value); a non-object settings root is rejected; the root-level `--session` (which clap
conflicts cannot reach) is rejected for `--enable`/`--disable` in dispatch. The `--auto`
flag also landed here with its disabled-is-a-silent-no-op behavior tested in
`tests/cli_contract.rs`; the enabled best-effort path is Task 4. Setup no longer opens the
settings file at all, so the broken-symlink refusal moved to `thread --enable`
(`thread_enable_refuses_a_broken_settings_symlink`). Validation: full suite green
(185 tests), fmt clean, clippy `-D warnings` clean. One transient failure of the known
timing-sensitive `doctor_waits_for_admission_and_preserves_the_new_live_route` under
first-build parallel load did not reproduce in two subsequent full runs.

### Task 4: `--auto` best-effort attach

**Covers:** R8, R9 (auto behavior), D3, D5

**Objective:** `zerdr thread --auto` is a silent no-op while disabled and a best-effort
attach while enabled, never blocking the thread's shell with a fatal error.

**Files:**
- Modify: `src/thread.rs`, `src/lib.rs`
- Test: `tests/thread_flow.rs`, `tests/cli_contract.rs`

**Dependencies:** Task 3 (flag file)

**Implementation notes:**
- Disabled check happens before any Herdr invocation or state locking; exit 0 with no output.
- Enabled: call the existing attach flow with `create=false`; on `Err`, print exactly one
  `zerdr: <message>` line to stderr and exit 0. Successful attach behaves identically to
  manual `zerdr thread` (blocks until detach, OSC titles, bell, leases).
- Multi-line guidance in existing errors (e.g. the no-matching-workspace hint) should
  collapse to a single line for the auto path; keep the manual path's wording unchanged.

**Test cases:**
- flag absent + `--auto` → exit 0, empty stdout/stderr, fake herdr records no calls.
- flag present + free agent fixture → attach happens (reuse an existing attach test with
  `--auto`), OSC title emitted.
- flag present + no matching workspace → exit 0, stderr is a single `zerdr: ` line, no
  Herdr workspace/tab created.
- flag present + herdr binary missing/failing → exit 0, single stderr line.
- `zerdr thread --auto TARGET` and `--auto --kind pi` → clap conflict errors;
  `--auto --session NAME` accepted.

**Complete when:**
- All auto-path outcomes (no-op, attach, best-effort failure) are covered by tests.
- Manual `zerdr thread` behavior is unchanged by the wrapper.
- Validation succeeds.

**Validation:**
- Run: `cargo test --test thread_flow --all-features && cargo test --all-targets --all-features`
- Expected: all tests pass.

**Result:** Done. `thread::run_auto` holds both the disabled check and the best-effort
wrapper (`zerdr: <one-line message>; starting a plain shell` on stderr, exit 0); lib.rs
dispatches `--auto` there. The disabled no-op and clap conflicts were already tested in
Task 3's `tests/cli_contract.rs`; the enabled paths (attach parity, unmatched workspace,
missing herdr binary) are in `tests/thread_flow.rs`. Validation: full suite green
(188 tests), fmt clean, clippy `-D warnings` clean.

### Task 5: Documentation

**Covers:** R13

**Objective:** README matches the reduced surface and documents the auto mode.

**Files:**
- Modify: `README.md`, `assets/zed/keymap.example.json`

**Dependencies:** Tasks 1–4

**Implementation notes:**
- Commands table: drop `pick`/`next`/`previous` and the `--mode`/`--focus` mentions; add
  the thread mode flags. Fix the setup row's "five global Zed tasks" to the single task
  (the Task 5 validation grep will not catch this). "Routing modes" section becomes a
  short internal-routing description (no location validation; running outside a Zed
  terminal simply routes and focuses Zed). Notes: drop the "migrated away by the next
  `zerdr setup`" sentence (uninstall still cleans up), drop "Stop that session's wrapper
  before changing its routing mode", and describe reattach-on-restart as accepted resume
  behavior.
- assets/zed/keymap.example.json: delete the four `task::Spawn` bindings for the removed
  Pick/Next/Previous/Sync Workspace tasks; keep the `terminal::SendText` binding and any
  binding spawning "zerdr: Herdr".
- Terminal threads section: manual attach stays the default story; auto mode is the
  opt-in paragraph (`zerdr thread --enable`, what `--auto` does on failure, `--disable`).

**Test cases:**
- N/A (prose). Verify command names in README against `zerdr --help` output.

**Complete when:**
- README contains no references to removed commands or flags.
- Validation succeeds.

**Validation:**
- Run: `rg -n "pick|previous|--mode|--focus" README.md`
- Expected: no matches referring to removed zerdr surface (unrelated words allowed).

**Result:** Done. README: Quickstart anchored on the Zed terminal, auto-mode paragraph in
Terminal threads, reduced command table (with the enable/disable row and the one-task
setup row), "Routing modes" collapsed into an internal-only "Routing" section, notes
updated (opt-in automation via `--enable`, reattach-on-restart, wrapper-ownership line
without the mode sentence). keymap example keeps only the `terminal::SendText` binding.
`rg "pick|previous|--mode|--focus|external|five" README.md` has no matches. AGENTS.md was
already updated alongside Tasks 1 and 3.

## Requirement Coverage

| Requirement / Decision | Task | Verification |
|---|---|---|
| R1 | Task 1 | removed-flag rejection tests; internal wrapper tests pass |
| R2 | Task 2 | unknown-subcommand tests; sync tests still pass |
| R3 | Task 2 | seeded stale-task cleanup and preserve-modified tests |
| R4 | Task 1 | `legacy_external_route_notifies_without_workspace_or_zed_calls` (added post-review) |
| R5 | Task 3 | flag file created/removed in enable/disable tests |
| R6 | Task 3 | enable tests: fresh, idempotent, foreign-value, symlink, no-install-state |
| R7 | Task 3 | disable tests: flag removed, settings untouched, idempotent |
| R8 | Task 4 | auto no-op, attach, and best-effort failure tests |
| R9 | Tasks 3, 4 | clap conflict tests for enable/disable/auto |
| R10 | Task 3 | setup-after-enable preserves fingerprint and settings |
| R11 | Task 3 | uninstall removes init command and flag |
| R12 | Task 3 | doctor enabled/disabled informational tests |
| R13 | Task 5 | README grep + manual read-through |
| D1–D5 | all | design constraints reflected in task notes (code review) |

## Final Validation

- [x] `cargo fmt --all -- --check` — Expected: no diff — clean
- [x] `cargo clippy --all-targets --all-features -- -D warnings` — Expected: no warnings — clean
- [x] `cargo test --all-targets --all-features` — Expected: all tests pass — 190 tests green
      (locally verified with `--test-threads=4` while the machine sat at load average
      11–13, where the two pre-existing timing-sensitive tests flake at full parallelism;
      GitHub CI on macOS and Ubuntu is green on HEAD 92163c4)
- [x] Manual check (user's machine, after `cargo install --path . --locked --force`):
      `zerdr thread --enable` once, open a new Zed terminal thread → auto-attaches;
      `zerdr thread --disable`, open another thread → nothing happens; `zerdr doctor`
      shows the mode line. Real `zerdr setup` run is the user's call per AGENTS.md.
- [x] Requirement Coverage に未対応項目がない
- [x] 計画と実際の変更内容が整合している
- [x] 上記のすべてが成功した後、計画を同名のまま `docs/plans/archived/` へ移した

**Independent review:** two rounds by a separate reviewer context. Round 1: APPROVED with
1 medium + 3 low findings. Round 2 (after fix commit 92163c4): APPROVED, all findings
closed — three fixed with tests, the medium install.json race accepted and recorded in
Risks. Implementation commits: c6b1977, 1a8bf98, 1b57c64, dd673b7, 49e65e7, 92163c4
(base cce7c09), all pushed and CI-green on HEAD.

## Risks and Open Questions

- A route-state file written by an older binary with external routing fails to parse after
  R1; it is reported as corrupt rather than cleaned automatically. Acceptable: routes only
  matter while a wrapper is live, and the user machine will not have a live external route.
- Known limitation (independent review, medium): `setup`, `thread --enable`/`--disable`,
  and `uninstall` mutate install.json without a shared lock, so a genuinely concurrent
  `setup` + `--enable` could revert the just-recorded init-command fingerprint. This
  extends a pre-existing unlocked pattern; interactive single-user usage makes the race
  unlikely, and adding cross-command locking was judged not worth the complexity now.
- `--auto` runs in every new terminal thread while enabled; if Herdr's CLI is slow to fail
  (e.g. hung socket), thread startup waits on it. Existing herdr calls have no zerdr-side
  timeout; unchanged by this plan.
- Restored threads reattach after Zed restart while the mode is on (accepted, D5); the
  thread-to-agent mapping may differ from before the restart.
- No open questions.
