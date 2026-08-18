# Anchor-Routed Zed Project Synchronization Implementation Plan

> **For implementers:** Execute tasks in order unless dependencies allow otherwise. Mark a task complete only after its validation succeeds. Reflect minor implementation differences in the relevant task. Ask the user before changing requirements, Out of Scope, or public contracts.

## Problem Statement

Real Zed 1.15.0 dogfooding disproved the original assumption that `zed --existing TARGET` adds an unknown checkout to the Zed window that hosts `zerdr herdr`. If TARGET is absent, Zed can open a new window; if TARGET is already open in another window, Zed focuses that window. A blank Zed window is not a usable `--add` destination. The MVP therefore needs an explicit project-backed bootstrap root to route each focus operation into one managed Zed window before adding and activating the selected Herdr checkout.

This plan revises the routing, wrapper cardinality, setup task count, diagnostics, documentation, and real-E2E portions of `docs/plans/2026-08-18-zerdr-mvp.md`. Unchanged binding, picker, notification, setup ownership, distribution, and release-approval contracts remain in force.

## Goal

Make normal Herdr workspace focus reliably perform the following sequence while one Zed-launched wrapper is live:

1. activate the managed Zed window through its current anchor project;
2. add or activate the selected canonical Git checkout in that window;
3. promote the selected checkout to the next dynamic anchor only after both Zed commands succeed.

The normal launch path is a generated `zerdr: Herdr` Zed task that passes the active `$ZED_WORKTREE_ROOT` as a required bootstrap anchor.

## Out of Scope

- Identifying or controlling a blank Zed window.
- Moving a project that is already open in another Zed window into the managed window.
- Supporting more than one live `zerdr herdr` wrapper.
- Keeping Zed project ordering equal to Herdr workspace ordering.
- Automatically removing Zed projects after a Herdr workspace closes.
- Automatically collapsing or expanding Zed project-panel entries.
- Reading Zed's project list, project order, active window ID, or project-panel state.
- Dispatching Zed UI actions through macOS Accessibility or undocumented internal APIs.
- Adding a Zed extension; the current public extension API cannot perform the required window/project operations.
- Using `zed --reuse`, replacing the managed Zed workspace, or modifying Zed/Herdr core.
- Automatically detecting that the current anchor was manually removed from Zed.
- Changing the approved release targets, Homebrew strategy, or explicit publication-approval gate.

## Requirements and Decisions

### Requirements

- **R1 — Explicit project-backed launch:** `zerdr herdr` requires `--anchor PATH`, `ZED_TERM=true`, and `TERM_PROGRAM=zed`. PATH is resolved through `git rev-parse --show-toplevel` and canonicalized before Herdr is spawned. There is no CWD fallback.
- **R2 — Zed task launch:** Setup owns a fifth task, `zerdr: Herdr`, whose command is the stable installed zerdr executable and whose arguments are `herdr --anchor $ZED_WORKTREE_ROOT`. It is a visible long-running task, does not hide automatically, and starts each candidate in a new terminal so zerdr can reject only a competing candidate while preserving the authoritative wrapper.
- **R3 — Single live wrapper:** At most one authoritative locked lease may exist for the fixed `zerdr` session socket. A concurrent second wrapper is rejected after socket discovery, its child client is terminated and reaped, and it never replaces route state.
- **R4 — Dynamic anchor state:** Each session/socket scope has versioned route state containing the canonical current anchor. The initial value is the explicit bootstrap anchor. The route record alone is not authority; a matching locked lease is still required.
- **R5 — Anchored focus pipeline:** Every startup, event, current-workspace selection, and explicit sync serializes by socket, revalidates the live lease, resolves the focused Herdr root, executes exactly `zed --existing CURRENT_ANCHOR` followed by `zed --add TARGET`, and writes TARGET as the new anchor only after both commands succeed.
- **R6 — On-demand project addition:** A previously absent TARGET is added and activated on first Herdr focus. Repeated `zed --add TARGET` is intentionally allowed because real Zed 1.15.0 proved it idempotent and focus-producing.
- **R7 — Failure behavior:** Root resolution, route-state, `zed --existing`, or `zed --add` failure leaves the previous anchor unchanged and sends the existing actionable Herdr notification. Herdr focus is not rolled back.
- **R8 — Nonfatal startup sync failure:** Once the child, socket, single lease, and bootstrap route are established, startup root/Zed synchronization failure is notified but does not terminate the Herdr UI or release the lease. The user can focus another workspace or run `zerdr bind PATH` and retry.
- **R9 — Anchor recovery contract:** After a successful focus, the formerly anchored project may be removed from Zed. The current anchor must remain in the managed Zed window until another successful focus promotes a replacement. If the current anchor is removed accidentally, the supported recovery is to stop the wrapper and relaunch `zerdr: Herdr` from a remaining project-backed Zed window.
- **R10 — Zed-window precondition:** Before launch, the user closes other Zed windows containing projects that Herdr will select. Zed does not duplicate one project across windows and `zed --add TARGET` focuses the other window when TARGET is already open there. This precondition is documented but cannot be diagnosed through the public CLI.
- **R11 — Existing command semantics:** `pick`, `next`, `previous`, `sync`, `bind`, and `unbind` retain their existing binding/preflight/task-delivery contracts. Focus-changing commands still call Herdr only; the plugin event owns the anchored Zed pipeline. Current selection, direct sync, and bind call that same pipeline directly.
- **R12 — Setup, doctor, and migration:** Re-running setup migrates a valid four-task installation to the exact five-task owned set without modifying keymaps or unrelated JSONC. Doctor validates both `zed --existing` and `zed --add`, the fifth task payload, route-state schema/path validity, the single-live-wrapper invariant, and the existing plugin/event contracts. With a live lease, a missing/malformed/mismatched route is blocking. Without a live lease, any route is stale and non-authoritative: doctor removes it and emits a warning rather than a blocking failure.
- **R13 — Quality and real validation:** All behavior is developed Red-Green-Refactor against fake command logs and real file locks. Real macOS E2E must prove the project-backed launch, on-demand add/focus, dynamic promotion, single-client rejection, recovery guidance, and no-new-window precondition before release approval.

### Implementation Decisions

- **D1 — No extension:** Supported Zed extensions cannot identify a window, mutate project membership, focus projects, dispatch arbitrary actions, or accept the required external request.
- **D2 — Two-command routing:** Use `--existing CURRENT_ANCHOR` before `--add TARGET`. A brief focus flash to the old anchor is accepted for the first implementation; optimization requires a separate user-approved behavior change after dogfooding.
- **D3 — Focus-driven reconciliation:** Subscribe only to `workspace.focused`. Do not subscribe to creation, close, or reorder events because ordering/removal reconciliation is out of scope and first focus performs the required add.
- **D4 — Dynamic rather than first-workspace anchor:** The most recently synchronized target becomes the anchor. This avoids lifecycle tracking for the first Herdr workspace and lets users remove the previous project after switching.
- **D5 — Socket-scoped route record:** Keep mutable route state separate from the lock-held lease record. Route reads and atomic writes occur under the existing socket `SyncGuard`.
- **D6 — Serialized admission:** Single-wrapper admission, initial route write, and lease acquisition occur under the same socket-scoped serialization lock used by sync. A race between two launchers produces one winner and one cleaned-up child.
- **D7 — Stale route is harmless:** Route state may remain after wrapper exit, but it cannot authorize events. The next launcher that observes no live lease replaces it with its explicit bootstrap anchor before startup sync.
- **D8 — Preserve bindings:** Dynamic anchor state does not replace the workspace-ID-to-checkout binding store. Bindings remain the stable source for target resolution.
- **D9 — User-owned Zed UI:** zerdr never removes, reorders, folds, or unfolds project-panel entries. Zed keeps those user-controlled states.

### Contracts

#### Public CLI

```text
zerdr herdr --anchor PATH
zerdr pick
zerdr next
zerdr previous
zerdr sync
zerdr bind [PATH]
zerdr unbind
zerdr setup
zerdr uninstall [--purge]
zerdr doctor
```

`--anchor` is required for `herdr`; nested and symlinked paths normalize to the canonical Git checkout root. Missing, nonexistent, or non-Git anchors fail before a child process is spawned.

#### Route state

One mutable record exists per canonical Herdr socket scope:

```json
{
  "schema_version": 1,
  "session_name": "zerdr",
  "socket_path": "/canonical/path/to/herdr.sock",
  "anchor_root": "/canonical/path/to/checkout",
  "wrapper_pid": 123
}
```

Invariants:

- `anchor_root` is an existing absolute canonical Git top-level whenever written;
- `socket_path` equals the canonical socket represented by the containing scope;
- while a live lease exists, unsupported schemas, malformed records, missing roots, or a wrapper PID inconsistent with that lease are actionable failures and are not overwritten by an event;
- without a live lease, a valid or malformed route is stale, non-authoritative state; doctor may remove it with a warning, and wrapper admission may replace it under the socket lock;
- successful sync atomically replaces only `anchor_root` while preserving the current session/socket/wrapper identity;
- uninstall without purge may preserve route/binding state; purge removes it only after proving no live lease exists.

#### Owned Zed tasks

Setup owns exactly five labels:

```text
zerdr: Herdr
zerdr: Pick Workspace
zerdr: Next Workspace
zerdr: Previous Workspace
zerdr: Sync Workspace
```

The new long-running task contract is:

```json
{
  "label": "zerdr: Herdr",
  "args": ["herdr", "--anchor", "$ZED_WORKTREE_ROOT"],
  "allow_concurrent_runs": true,
  "use_new_terminal": true,
  "reveal": "always",
  "hide": "never"
}
```

The generated `command` remains the setup-time stable executable. Existing picker/navigation/sync task payloads retain their prior reveal, hide, and notification-delivery contracts. Setup records all five fingerprints and safely upgrades a prior zerdr-owned four-task install.

#### Synchronization transition

```text
workspace.focused / startup / current selection / explicit sync
  -> acquire socket SyncGuard
  -> matching single locked lease still live?
     no: existing no-op/error contract by caller type
     yes:
       read and validate route state
       re-read focused Herdr workspace
       resolve canonical TARGET through binding rules
       zed --existing CURRENT_ANCHOR
       zed --add TARGET
       atomically promote route.anchor_root = TARGET
       return TARGET
```

No route promotion occurs if any preceding step fails. Plugin/manual error delivery remains as defined by the MVP plan.

#### Wrapper transition

```text
validate/canonicalize explicit bootstrap anchor
  -> spawn Herdr client and discover fixed-session socket
  -> acquire socket SyncGuard
  -> live lease exists?
     yes: reject second wrapper; terminate/reap this child
     no: atomically write bootstrap route; acquire one locked lease
  -> release SyncGuard
  -> startup sync
     success: target becomes dynamic anchor
     failure: notify, keep bootstrap route + lease + Herdr UI alive
  -> wait for child; on exit remove lease only, never stop session
```

## Current Context

### Confirmed

- The current implementation passes 43 local automated tests before this revision and has setup, binding, lease, notification, release, and documentation foundations in place.
- Real dogfooding used Zed 1.15.0, Herdr 0.8.0, and zerdr 0.1.0.
- `zed --existing TARGET` focuses an existing matching project; when TARGET is absent it can open a new Zed window instead of adding to the Herdr-hosting window.
- A blank Zed window is not a valid `zed --add` destination; `zed --add TARGET` from it opens a new window.
- In a project-backed foreground Zed window, `zed --add TARGET` adds TARGET, activates it, and is idempotent on repetition.
- When TARGET is already open in another Zed window, `zed --add TARGET` focuses that other window. Zed does not allow the same project to be open in two windows through this flow.
- Reissuing multiple paths in a different order does not reorder existing Zed projects.
- Zed exposes project-panel collapse actions internally, but no public CLI/extension API can dispatch “collapse every inactive project.”
- Zed tasks support `$ZED_WORKTREE_ROOT`, `allow_concurrent_runs`, long-running terminal reveal/hide controls, and stable command arguments.
- The existing Herdr plugin subscribes to `workspace.focused`, and current fake-process seams record exact Zed command order/arguments.

### Assumptions

- Internal type names for the route store and admission guard may follow existing `state.rs` naming patterns as long as the persisted contract and observable behavior above remain unchanged.

## File Structure

- Create: `docs/plans/2026-08-18-anchor-routed-zed-sync.md` — this routing revision and progress record.
- Modify: `docs/plans/2026-08-18-zerdr-mvp.md` — mark the original direct-`--existing` assumptions as superseded and record the failed real E2E.
- Modify: `src/cli.rs` — required `herdr --anchor PATH` contract.
- Modify: `src/lib.rs` — pass the explicit anchor into wrapper orchestration.
- Modify: `src/state.rs` — socket-scoped route schema/store, atomic promotion, and single-wrapper admission support.
- Modify: `src/herdr.rs` — anchor-aware single-client wrapper initialization and nonfatal startup-sync handling.
- Modify: `src/zed.rs` — separate exact `--existing` and current-workspace `--add` operations with capability checks.
- Modify: `src/sync.rs` — anchored two-command focus pipeline and success-only dynamic promotion.
- Modify: `src/setup.rs` — five-task ownership/migration.
- Modify: `src/doctor.rs` — `--add`, route, and single-live-wrapper diagnostics.
- Modify: `assets/zed/tasks.json.in` — long-running `zerdr: Herdr` task using `$ZED_WORKTREE_ROOT`.
- Modify: `assets/zed/keymap.example.json` — optional Herdr task binding only if the existing example format remains concise; setup still does not install keybindings.
- Modify: `tests/cli_contract.rs` — required anchor and pre-spawn validation.
- Modify: `tests/state_and_bindings.rs` — route schema, atomicity, stale replacement, and admission locking.
- Modify: `tests/herdr_wrapper.rs` — bootstrap route, one-client rejection, cleanup, and nonfatal startup failure.
- Modify: `tests/sync_flow.rs` — exact anchored command sequence, add idempotency seam, promotion, and failure rollback.
- Modify: `tests/setup_and_doctor.rs` — five-task migration/ownership and diagnostics.
- Modify: `tests/support/mod.rs` — fake Zed `--add` capability and deterministic command/failure controls.
- Modify: `README.md` — task-first launch, project-backed/other-window preconditions, dynamic-anchor removal/recovery, and explicit non-goals.

## Testing Decisions

- **Test seam:** Continue running the built CLI against fake Herdr/Zed executables and isolated state roots. Assert exact process order and persisted route state rather than private helper calls.
- **State/concurrency:** Use real file locks and child processes to race two wrappers for one socket and prove one lease/route owner.
- **Zed behavior:** Fake `--existing` and `--add` independently so either phase can fail. Real GUI behavior remains a manual macOS gate because window/project state is not queryable.
- **Migration:** Start fixtures from the current four-task install state, rerun setup with the revised binary, and prove one Herdr task is added while existing owned/unrelated JSONC remains correct.
- **Prior art:** Reuse `queued_event_rechecks_lease_after_acquiring_sync_lock`, wrapper child-cleanup tests, setup rollback/fingerprint tests, and fake command logging.
- **Avoid:** Do not encode project ordering, project removal, panel folding, Zed internal DB state, GUI automation, or implementation-specific route helper call graphs in tests.

## Progress

- [x] Task 1: Establish explicit anchor, route-state, and single-wrapper contracts.
- [x] Task 2: Deliver anchored on-demand add/focus synchronization.
- [x] Task 3: Migrate setup, doctor, and public documentation.
- [x] Task 4: Complete automated regression and revised real macOS E2E validation.

Implementation-time minor file changes or internal differences must be reflected in the relevant task. Ask the user before changing requirements, Out of Scope, public contracts, persisted schemas, or task labels.

Implementation record:

- Added one global lifecycle file lock outside purgeable zerdr directories. Wrapper admission, doctor route cleanup, and `uninstall --purge` hold it while checking or mutating authority state.
- Sync now requires exactly one locked lease whose wrapper PID matches the route before any Herdr workspace resolution or Zed invocation.
- Lease sweeps retain live socket hashes so doctor removes stale routes per scope while preserving unrelated live routes.
- Automated coverage includes simultaneous launcher admission/loser reaping, doctor/admission and purge/admission races, four route-corruption classes, same-anchor repeat focus, and live lease retention after nonfatal startup failure.
- Real Zed 1.15.0 testing showed that `allow_concurrent_runs: false` terminates the authoritative long-running task before restarting it, while `true` alone still reuses and replaces the existing terminal. The generated task therefore combines `allow_concurrent_runs: true` with `use_new_terminal: true`; zerdr's race-safe admission can then reject the candidate while keeping the first UI, lease, and route authoritative.

## Tasks

### Task 1: Establish Explicit Anchor, Route State, and Single-Wrapper Contracts

**Covers:** R1–R4, R9, D5–D8

**Objective:** Make wrapper launch unambiguous, persist one authoritative dynamic route per live socket, and reject concurrent wrappers without leaking child processes.

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/lib.rs`
- Modify: `src/state.rs`
- Modify: `src/herdr.rs`
- Modify: `tests/cli_contract.rs`
- Modify: `tests/state_and_bindings.rs`
- Modify: `tests/herdr_wrapper.rs`
- Modify: `tests/support/mod.rs`

**Dependencies:** Existing MVP Tasks 1–3.

**Implementation notes:**
- Start with failing CLI tests for missing/non-Git anchor and route/admission state tests.
- Canonicalize the anchor before spawning Herdr so invalid input has no process or state side effect.
- Serialize no-live check, bootstrap route replacement, and lease acquisition under the socket lock.
- Preserve cleanup-on-error behavior when a second launcher loses admission after spawning its child.
- Keep route authority subordinate to the locked lease; stale route files never authorize a plugin event.
- Convert existing multi-client tests to explicit single-client rejection tests.

**Test cases:**
- `zerdr herdr` without `--anchor` → Clap usage failure before Herdr/Zed calls.
- Nested or symlinked Git path → canonical top-level stored as bootstrap anchor.
- Missing/non-Git anchor → actionable failure, no child, lease, or route.
- First wrapper after no live lease → one canonical route plus one locked lease.
- Existing stale route with no lease → next wrapper replaces it with the explicit bootstrap.
- Two wrappers race after socket discovery → one remains live; loser child is terminated/reaped; route belongs to winner; no second lease remains.
- Unsupported/malformed route or route/socket mismatch during an event → notification and no Zed call.
- Wrapper exit → lease removed, route may remain but `has_live` is false and events no-op.

**Complete when:**
- Public CLI and persisted route schema are fixed by tests.
- Single-wrapper admission is race-safe and child-cleanup tests pass.
- Existing session preservation and signal/exit propagation remain intact.

**Validation:**
- Run: `cargo test --test cli_contract && cargo test --test state_and_bindings && cargo test --test herdr_wrapper`
- Expected: all anchor parsing, canonicalization, route, admission, lifecycle, and existing regression cases pass.

### Task 2: Deliver Anchored On-Demand Add/Focus Synchronization

**Covers:** R5–R9, R11, D2–D5, D8

**Objective:** Route every eligible sync to the managed window, add/activate only the selected target, and promote the anchor only after complete success.

**Files:**
- Modify: `src/zed.rs`
- Modify: `src/sync.rs`
- Modify: `src/herdr.rs`
- Modify: `tests/sync_flow.rs`
- Modify: `tests/herdr_wrapper.rs`
- Modify: `tests/support/mod.rs`

**Dependencies:** Task 1.

**Implementation notes:**
- Write failing fake-log tests that require `--existing OLD` immediately followed by `--add TARGET`.
- Keep focus-event serialization and post-lock lease revalidation.
- Read route state and re-read focused Herdr workspace under the sync lock.
- Resolve only the focused target; do not enumerate/add every Herdr workspace.
- Atomically promote TARGET after `--add` succeeds. Preserve OLD on resolution, existing-phase, add-phase, or state-write failure.
- Startup sync failure after admission uses existing notification delivery but returns control to the wrapper wait loop instead of killing the child.
- Preserve the task-mode rule that notification-delivered errors may hide navigation/sync terminals while direct CLI errors remain nonzero.

**Test cases:**
- Eligible bound event with anchor A and target B → exact log order `zed --existing A`, `zed --add B`; route becomes B.
- Target equals anchor → commands remain idempotent and route stays canonical.
- Unbound focused workspace → target resolves/binds first, then anchored sequence runs and promotes it.
- Unrelated unbound workspaces → no snapshot/root discovery.
- Existing phase failure → no add call and anchor remains A; notification emitted.
- Add phase failure → anchor remains A; notification emitted.
- Route atomic-write failure after successful Zed commands → reported, old valid route remains readable.
- Queued event whose lease expires → no Zed call or promotion.
- `pick`/next/previous to another workspace → command issues Herdr focus only; plugin performs one anchored sequence.
- Current pick, sync, and bind → shared anchored sequence directly.
- Startup root/Zed failure → notification recorded, wrapper child remains until its normal test exit, lease remains live during that interval, route remains bootstrap A.

**Complete when:**
- All sync entry points use the two-command route and success-only promotion.
- Existing binding, picker, navigation, notification, and no-live semantics do not regress.
- Focused and full sync tests pass.

**Validation:**
- Run: `cargo test --test sync_flow && cargo test --test herdr_wrapper`
- Expected: exact command ordering, dynamic promotion, rollback, nonfatal startup, and all existing sync regressions pass.

### Task 3: Migrate Setup, Doctor, and Public Documentation

**Covers:** R2, R9–R12, D1, D9

**Objective:** Make task-first project-backed launch discoverable, migrate existing installs safely, and diagnose every enforceable prerequisite without claiming visibility into Zed window state.

**Files:**
- Modify: `assets/zed/tasks.json.in`
- Modify: `assets/zed/keymap.example.json`
- Modify: `src/setup.rs`
- Modify: `src/doctor.rs`
- Modify: `src/zed.rs`
- Modify: `tests/setup_and_doctor.rs`
- Modify: `tests/support/mod.rs`
- Modify: `README.md`
- Modify: `docs/plans/2026-08-18-zerdr-mvp.md`

**Dependencies:** Tasks 1–2.

**Implementation notes:**
- Add the exact fifth owned task and include it in setup fingerprints, conflict detection, uninstall, rollback, and doctor payload validation.
- Use `$ZED_WORKTREE_ROOT` literally in task args; do not resolve it during setup.
- Keep keymap user-owned. If a launch binding is shown, it remains an example and setup never writes `keymap.json`.
- Doctor checks help output for both `--existing` and `--add`. With a live lease it requires one valid matching route and reports more than one live lease as blocking. With no live lease it removes any valid or malformed stale route with a warning and does not treat stale route contents as blocking.
- Doctor cannot assert that an anchor is open in exactly one Zed window or that other project windows are closed; report those as documented manual preconditions, not false PASS claims.
- README removes the old direct bare `zerdr herdr` quick start and explains task-first launch, explicit direct launch, dynamic-anchor removal order, restart recovery, and out-of-scope order/removal/folding behavior.

**Test cases:**
- Fresh setup → exactly five owned tasks, including long-running Herdr task with anchor variable and no keymap mutation.
- Existing valid four-task zerdr install → setup adds the fifth and refreshes fingerprints without duplicating or altering unrelated JSONC.
- Repeated revised setup → byte-stable/idempotent result.
- Modified/foreign fifth label → same preservation/conflict rules as existing labels.
- Uninstall → removes only current fingerprint-matching five tasks; user-modified tasks remain.
- Zed help missing `--add` → doctor blocking failure with upgrade guidance.
- Valid live route → doctor reports canonical anchor and one live wrapper.
- Missing/malformed route while lease is live, or multiple live leases → blocking route/lifecycle diagnosis.
- Valid or malformed route with no live lease → doctor removes it, warns that stale route state was cleaned, and has no route-related blocking failure.
- README commands and generated task payload → match `zerdr --help` and setup output.

**Complete when:**
- Existing dogfood installation upgrades without manual config edits.
- Setup/uninstall rollback and ownership guarantees still pass.
- Doctor distinguishes enforceable checks from manual Zed-window preconditions.
- Public docs no longer claim that `--existing TARGET` alone safely adds an absent project.

**Validation:**
- Run: `cargo test --test setup_and_doctor && cargo test --test cli_contract`
- Expected: five-task migration, ownership, doctor capability/route checks, and CLI documentation contracts pass.
- Run: `rg -n 'zerdr: Herdr|--anchor|other Zed windows|project order|collapse|remove.*anchor|restart' README.md`
- Expected: README itself documents task-first launch, the other-window precondition, dynamic-anchor removal/restart recovery, and the ordering/folding exclusions; matches from the plan cannot satisfy this check.

### Task 4: Complete Automated Regression and Revised Real macOS E2E Validation

**Covers:** R1–R13, D1–D9

**Objective:** Prove the revised release candidate locally and in real Zed/Herdr before resuming distribution validation or requesting publication approval.

**Files:**
- Modify: `docs/plans/2026-08-18-anchor-routed-zed-sync.md` — record automated and manual results.
- Modify: `docs/plans/2026-08-18-zerdr-mvp.md` — align revision status and release gate.

**Dependencies:** Tasks 1–3.

**Implementation notes:**
- Use disposable Git checkouts and a project-backed Zed window.
- Close other Zed windows that contain any disposable Herdr target before launch.
- Do not publish, tag, or mutate the Homebrew tap.
- Record exact Zed, Herdr, and zerdr versions and each visual observation.

**Test cases:**
- Revised setup twice → exactly five tasks, unchanged keymap/unrelated tasks, doctor no blocking installation/capability failures.
- Run `zerdr: Herdr` from a project-backed Zed window → one wrapper/session/lease opens and startup focus remains in that window.
- Run bare `zerdr herdr` or use a non-Git anchor → rejected before child launch.
- Focus an absent disposable Herdr checkout → it is added and active in the managed window; no new Zed window opens.
- Focus the same checkout again → no duplicate project; it becomes active.
- Switch A→B successfully, remove A from Zed, then switch again → dynamic anchor B continues routing correctly.
- Remove current anchor B intentionally → documented restart flow from a remaining project restores synchronization.
- Launch `zerdr: Herdr` a second time while live → second task fails visibly; first UI/lease/route remains authoritative.
- Focus an invalid/non-Git Herdr workspace at startup → notification appears, Herdr UI remains usable, valid later focus recovers.
- Leave another Zed window holding TARGET as a negative precondition check → Zed focuses that other window as documented; zerdr does not claim to prevent or detect it.
- Project ordering, closed-workspace removal, and panel folding → remain user-controlled and unchanged by zerdr.

**Complete when:**
- All automated gates pass after the revision.
- Every revised manual case has an observed result recorded here.
- The original MVP release gate points to this completed revision. If this plan is archived, update that pointer to `docs/plans/archived/2026-08-18-anchor-routed-zed-sync.md` in the same documentation-only move.
- Any remaining `dist`/hosted-CI/publication items remain explicitly unverified rather than being marked complete.

**Validation:**
- Run: `cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-targets --all-features`
- Expected: no format/Clippy failures and all revised plus existing tests pass.
- Run: `cargo metadata --no-deps --format-version 1 >/dev/null && cargo package --allow-dirty --no-verify --list >/dev/null && actionlint .github/workflows/*.yml && git diff --check`
- Expected: metadata/package/workflows/diff checks pass.
- Run: the manual cases above in Zed 1.15.0 and Herdr 0.8.0.
- Expected: anchored on-demand focus passes under the precondition and each unsupported condition matches documentation.

### Real macOS E2E results

Validated on 2026-08-18–19 with Zed 1.15.0, Herdr 0.8.0, and zerdr 0.1.0 using `/private/tmp/zerdr-dogfood.IhMWQ4/{alpha,beta}`:

- Revised setup was repeated successfully: exactly five owned tasks remained, doctor had no blocking failure, and `keymap.json` remained absent.
- `zerdr: Herdr` launched from the project-backed zerdr window with one live wrapper and a valid bootstrap route.
- Bare `zerdr herdr` and `--anchor /tmp` were rejected before a usable child launch.
- Focusing absent alpha and beta added each checkout to the same Zed window and activated the matching Herdr shell.
- Re-focusing alpha reused the existing project without duplication.
- After switching beta → alpha and removing beta, switching to zerdr still routed through the promoted alpha anchor.
- Removing the current alpha anchor broke its eligibility as documented; closing the wrapper, relaunching from remaining zerdr, and focusing alpha added it again in the managed window.
- Real Zed task behavior required both `allow_concurrent_runs: true` and `use_new_terminal: true`. With the final payload, the first Herdr task remained live and a second terminal failed visibly with `already has a live wrapper` and exit code 1.
- Temporarily moving beta's `.git` produced a visible sync notification both on focus and startup while Herdr remained usable. Restoring `.git`, then focusing zerdr and beta, recovered and added beta normally.
- When beta was already open in another Zed window, focusing beta moved focus to that other window, matching the documented negative precondition.
- Project insertion order, manual project removal, and panel folding remained Zed/user-controlled; zerdr did not reorder, remove, or fold projects.

## Requirement Coverage

| Requirement / Decision | Task(s) | Verification |
|---|---|---|
| R1 | 1, 4 | CLI pre-spawn anchor tests; direct-launch E2E |
| R2 | 3, 4 | Exact five-task payload/migration tests; real task launch |
| R3, D6 | 1, 4 | Real lock race and child cleanup; second-task E2E |
| R4, D5, D7 | 1, 2 | Route schema/stale replacement tests; success-only promotion |
| R5, D2 | 2, 4 | Exact fake command order; visible anchored routing E2E |
| R6, D3, D4 | 2, 4 | Repeat-add logs and real absent/repeat focus checks |
| R7 | 2 | Existing/add/state-write failure and unchanged-anchor assertions |
| R8 | 2, 4 | Nonfatal startup test and invalid-workspace recovery E2E |
| R9 | 1–4 | Dynamic promotion assertions, README procedure, removal/restart E2E |
| R10 | 3, 4 | Documented manual precondition and negative real-window check |
| R11, D8 | 2 | Existing picker/navigation/bind regressions plus anchored logs |
| R12 | 3 | Four-to-five task migration, doctor route/capability fixtures |
| R13 | 1–4 | TDD-focused suites, full gates, and revised real E2E |
| D1 | 3 | Documentation states extension/API boundary without extension files |
| D9 | 3, 4 | README exclusions and E2E observation that UI state is untouched |

## Final Validation

- [x] `cargo test --test cli_contract && cargo test --test state_and_bindings && cargo test --test herdr_wrapper` — Passed explicit anchor, route, single-wrapper, and lifecycle contracts.
- [x] `cargo test --test sync_flow && cargo test --test setup_and_doctor` — Passed anchored two-command sync, promotion/rollback, five-task migration, route-corruption, and doctor concurrency contracts.
- [x] `cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-targets --all-features` — Passed all local quality gates with 67 tests.
- [x] `cargo metadata --no-deps --format-version 1 >/dev/null && cargo package --allow-dirty --no-verify --list >/dev/null` — Passed package metadata and contents validation.
- [x] `actionlint .github/workflows/*.yml && git diff --check` — Passed workflow and changed-file static validation.
- [x] Revised manual macOS E2E checklist — Passed and recorded above with Zed 1.15.0 and Herdr 0.8.0.
- [x] Original MVP plan points to this revision and contains no unqualified release-readiness claim based on the failed direct-`--existing` E2E.
- [x] Requirement Coverage has no unmapped requirement or decision.
- [x] The plan and actual changed-file set agree, including documented lifecycle locking and final Zed task terminal semantics.
- [x] No release, tag, push, or Homebrew tap mutation occurred without explicit approval.
- [x] After every item above succeeds, update the original MVP plan's revision pointer to the archived path and move this file unchanged in name to `docs/plans/archived/2026-08-18-anchor-routed-zed-sync.md`.

## Risks and Open Questions

### Risks

- Zed exposes no window/project query API. If the current anchor is manually removed or TARGET is open in another Zed window, Zed may route elsewhere while returning success; zerdr cannot detect that outcome.
- The two-command pipeline may visibly flash the previous anchor before activating TARGET. This is accepted for the first revision and remains a dogfooding observation, not an automatic scope expansion.
- `$ZED_WORKTREE_ROOT` only resolves in project context. The Herdr task must be unavailable or fail clearly outside a project-backed Zed window.
- Route updates and wrapper admission cross process boundaries; missing socket serialization could admit two owners or allow stale promotion.
- Real project membership, activation, and no-new-window behavior cannot be asserted in CI and remain release-blocking manual checks.

### Open Questions

None. Changing the single-wrapper rule, two-command sequence, anchor recovery, Zed-window precondition, task label set, or excluded project-order/removal/folding behavior requires user confirmation and a plan update before implementation.
