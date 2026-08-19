# Herdr One-Shot Zed Action Implementation Plan

> **For implementers:** Execute tasks in order unless dependencies allow otherwise. Mark a task complete only after its validation succeeds. Reflect minor implementation differences in the relevant task. Ask the user before changing requirements, Out of Scope, or public contracts.

## Problem Statement

zerdr currently couples Herdr workspace focus with Zed routing whenever the bare `zerdr` wrapper owns the dedicated `zerdr` session. This follow mode remains useful, but ordinary Herdr use also needs an explicit operation that opens only the selected workspace in Zed without starting continuous synchronization.

Herdr 0.8.0 can invoke workspace-scoped plugin actions and lets users bind those actions to configurable keys. zerdr should add an `Open Zed` action for this one-shot workflow while preserving the existing wrapper, routing, installation, and safety contracts.

## Goal

Deliver two complementary behaviors through one installed zerdr plugin:

- bare `zerdr` continues to launch the dedicated session and automatically route the initially focused and subsequently focused workspaces;
- any local Herdr session can invoke a workspace-scoped `Open Zed` action, normally through a user-configured `prefix+z`, to open that action's workspace and leave Zed foreground without starting follow mode.

Make bindings session-aware so explicit workspace mappings remain safe across ordinary Herdr sessions, and allow `bind`/`unbind` to repair those mappings without a live wrapper.

## Out of Scope

- Adding plugin actions to Herdr's right-click context menu or modifying Herdr itself.
- Automatically editing the user's Herdr `config.toml` or choosing a mandatory keybinding.
- Adding a public `zerdr open` command or changing `zerdr sync` into a one-shot fallback.
- Disabling, replacing, or making optional the existing bare-wrapper follow behavior.
- Splitting setup into wrapper and plugin-only installation modes or publishing a separately packaged Herdr plugin.
- Supporting non-Git workspace directories, SSH, WSL, containers, dev containers, or other remote checkout routing.
- Falling back to plain Zed opening when an applicable live wrapper route is malformed, ambiguous, or owned by the wrong process.
- Changing the existing Zed `--existing` and `--add` capability baseline.
- Adding release tags, GitHub releases, or Homebrew tap changes.

## Requirements and Decisions

### Requirements

- **R1 — Existing follow mode:** Bare `zerdr` retains its current startup sync, `workspace.focused` event handling, internal/external routing, focus policy, fixed `zerdr` session, and one-live-wrapper invariant.
- **R2 — Plugin action:** The generated plugin declares one workspace-scoped action with title `Open Zed`, local action id `open-zed`, and a command that invokes a hidden zerdr entrypoint. The existing focus event remains in the same manifest.
- **R3 — Arbitrary local sessions:** The action uses its injected `HERDR_SOCKET_PATH` and action context rather than the fixed `zerdr` session, and works in any named local Herdr session including `default`.
- **R4 — Exact action target:** The action uses the `workspace_id` captured in `HERDR_PLUGIN_CONTEXT_JSON`; it must not replace that target with whichever workspace is focused later.
- **R5 — Root resolution:** An explicit binding for the action's session and workspace wins. Without one, the first present candidate is authoritative: use `worktree.checkout_path` when present, otherwise `workspace_cwd`, and resolve it as a canonical Git root for that invocation only. Do not create a binding or fall through to `workspace_cwd` when a present checkout path is invalid. A missing, stale, noncanonical, or non-Git target fails rather than opening another directory.
- **R6 — Route-aware one-shot:** Under the target socket's sync lock, inspect lease authority. With no live wrapper for that socket, invoke plain `zed TARGET`. With exactly one live wrapper and a matching route, use the existing route strategy but force the result to leave Zed foreground. Multiple live wrappers, missing/malformed route state, PID mismatch, or invalid internal anchor fail without a plain-open fallback.
- **R7 — Live route behavior:** A live internal route runs the current `--existing ANCHOR` then `--add TARGET` pipeline and promotes only after success. A live external route runs `--existing TARGET` without terminal-focus restoration, regardless of its persisted focus policy.
- **R8 — Foreground behavior:** A valid action always represents an explicit transition to Zed. It never restores the previously foreground application and never starts or acquires a wrapper lease.
- **R9 — Action errors:** Failures after a valid action session is known produce a Herdr notification, return nonzero, and remain visible in Herdr plugin logs. Malformed invocations that cannot identify a session still return nonzero through stderr/plugin logging.
- **R10 — Session-aware bindings:** Binding state keys mappings by Herdr session name and workspace ID. Legacy schema-v1 state is read compatibly as mappings under session `zerdr` and is atomically rewritten as schema v2 on the next successful binding mutation; read-only operations do not rewrite it.
- **R11 — Wrapper-optional binding commands:** `zerdr bind` and `zerdr unbind` can mutate the selected binding without a live wrapper. If one valid wrapper owns the target socket, `bind` preserves its current post-write synchronization behavior; without one it only persists state. `unbind` does not trigger synchronization, matching current behavior.
- **R12 — Session selection for binding commands:** `bind` and `unbind` gain `--session NAME`. An explicit option targets the named session's focused workspace. Without it, a complete Herdr pane context (`HERDR_SOCKET_PATH` plus `HERDR_WORKSPACE_ID`) targets that exact workspace; otherwise commands retain the backward-compatible dedicated `zerdr` session and its focused workspace. Partial context is rejected rather than mixed with another session.
- **R13 — Unified setup and configurable key:** `zerdr setup` continues to install the plugin plus all five owned Zed tasks. It prints a Herdr keybinding example for `zerdr.open-zed` using `prefix+z`, but never edits Herdr configuration.
- **R14 — Upgrade compatibility:** Bare wrapper launch eligibility continues to require only the compatible enabled focus event and current executable identity. A pre-action manifest from an older setup does not block follow mode. Setup and doctor separately detect whether the new action is installed and direct the user to rerun `zerdr setup` when it is absent or malformed.
- **R15 — Diagnostics:** No live wrapper and no corrupt live authority is a valid plugin-only state, not a warning. Doctor reports one-shot availability, validates action installation and every session-scoped binding, and retains strict failures for corrupt/multiple live wrapper state.
- **R16 — Platform and capability boundary:** Existing local-environment rejection applies to the hidden action and public binding commands. The current Zed `--existing`/`--add` baseline remains an installation requirement enforced and reported by doctor/documentation; this change does not add a new per-action or per-binding capability probe. Plugin-only use does not relax the diagnosed Zed version requirement.
- **R17 — Existing command compatibility:** `pick`, `next`, `previous`, and `sync` remain wrapper-authorized commands against the dedicated `zerdr` session. The hidden action does not appear in help, and launch-only options remain incompatible with all subcommands.

### Implementation Decisions

- **D1 — Runtime mode, not install mode:** The same plugin manifest serves both workflows. A live lease selects route-aware behavior; no live lease selects plain one-shot opening.
- **D2 — Snapshot action context:** Parse the Herdr 0.8.0 invocation context into a typed internal value and resolve the captured workspace directly. Do not call the current `sync_socket` path that rereads focus.
- **D3 — Per-socket arbitration:** Hold the existing socket `SyncGuard` while deciding live-route versus plain-open behavior and through the selected Zed operation. This prevents wrapper admission from racing the fallback decision.
- **D4 — Explicit mapping remains authoritative:** Existing bindings are never silently bypassed or replaced by action context, even when the context contains a valid alternate checkout.
- **D5 — Read-compatible, write-migrated state:** Readers accept both binding schemas. Mutators acquire the existing binding lock, reread under that lock, validate legacy bytes, and write only schema v2 atomically. Corrupt or unsupported bytes remain unchanged.
- **D6 — Context-aware Herdr adapter:** Preserve fixed-session methods for wrapper commands and add a session-targeted boundary for action notifications/workspace access and `--session` binding operations. Resolve an action's session name by matching its canonical socket against `herdr session list --json`.
- **D7 — Separate compatibility predicates:** Launcher preflight validates only what follow mode needs. Full installation/doctor validation additionally requires the exact action declaration, avoiding a binary-only upgrade regression.

### Contracts

#### Public CLI additions

```text
zerdr bind [--session NAME] [PATH]
zerdr unbind [--session NAME]
```

Session resolution is deterministic:

1. explicit `--session NAME` selects that running session and its focused workspace;
2. otherwise, both `HERDR_SOCKET_PATH` and `HERDR_WORKSPACE_ID` select the exact current Herdr workspace;
3. otherwise, use the focused workspace in the named `zerdr` session;
4. one context variable without the other is an actionable error.

No other public command gains `--session`. `bind` without a live wrapper writes the canonical root and exits without invoking Zed. A valid same-socket wrapper keeps the existing bind-then-sync behavior and no-rollback semantics if Zed fails.

#### Plugin action manifest

```toml
[[actions]]
id = "open-zed"
title = "Open Zed"
contexts = ["workspace"]
command = ["<setup-resolved-zerdr-executable>", "open-from-herdr"]
```

The hidden command requires `HERDR_PLUGIN_ACTION_ID=open-zed`, `HERDR_SOCKET_PATH`, and an object-valued `HERDR_PLUGIN_CONTEXT_JSON` with `workspace_id`. Resolve the session from a valid socket before validating the remaining context so context failures can be notified when possible. Candidate roots come from Herdr 0.8.0 fields in this strict order after binding lookup:

1. use `worktree.checkout_path` when the field is present;
2. otherwise use `workspace_cwd`.

A present but invalid first candidate is an error, not permission to try the next field.

The suggested user configuration is:

```toml
[[keys.command]]
key = "prefix+z"
type = "plugin_action"
command = "zerdr.open-zed"
description = "open workspace in Zed"
```

#### Binding state

New writes use schema v2:

```json
{
  "schema_version": 2,
  "sessions": {
    "default": {
      "w0": "/canonical/default/checkout"
    },
    "zerdr": {
      "w1": "/canonical/zerdr/checkout"
    }
  }
}
```

Invariants:

- session names and workspace IDs are independent map keys; identical workspace IDs in different sessions do not collide;
- stored paths are canonical Git checkout roots;
- schema-v1 `{schema_version, session_name: "zerdr", bindings}` loads as the equivalent `sessions.zerdr` map;
- a read does not mutate v1 bytes;
- the first successful bind, unbind, or other binding write serializes the complete state as v2 under the exclusive binding lock;
- malformed, unsupported, or legacy state naming a session other than `zerdr` is rejected without overwrite;
- concurrent migration and updates cannot lose mappings from either session.

#### One-shot routing state machine

```text
validate local environment + action context
  -> resolve session name from action socket
  -> resolve captured workspace root without implicit binding
  -> acquire socket SyncGuard
  -> inspect leases for that socket
       no live wrapper:
         zed TARGET
       exactly one live wrapper:
         load route and require route PID = wrapper PID
         internal: zed --existing ANCHOR -> zed --add TARGET -> promote
         external: zed --existing TARGET (no focus restoration)
       multiple wrappers or invalid authority:
         notify + fail; no Zed fallback
```

The root may be resolved before acquiring the socket lock, but route selection and the Zed call remain under the lock. A workspace focus change after action invocation does not change the captured `workspace_id` or root candidate.

## Current Context

### Confirmed

- `assets/herdr/herdr-plugin.toml.in` currently declares only the `workspace.focused` event and hidden `sync-from-herdr` command.
- `Synchronizer::event` already treats no live lease as a successful no-op, so the event can remain enabled during plugin-only use.
- `Synchronizer::sync_socket` currently authenticates one lease and route under `SyncGuard`, but rereads the currently focused workspace and applies persisted external focus restoration; the action cannot reuse it unchanged.
- `Herdr` process helpers currently hard-code `--session zerdr`; action execution receives `HERDR_SOCKET_PATH`, `HERDR_WORKSPACE_ID`, and a richer `HERDR_PLUGIN_CONTEXT_JSON` in Herdr 0.8.0.
- Herdr 0.8.0 action context exposes `workspace_id`, `workspace_cwd`, and `worktree.checkout_path`, and supports `type = "plugin_action"` keybindings.
- `Zed` currently exposes only `activate_existing`, `add_to_current`, and capability inspection; plain `zed TARGET` needs a process-boundary method.
- Binding schema v1 stores one top-level `session_name = "zerdr"` and one workspace map. Mutators use an exclusive lock plus atomic write, while reads are currently unlocked.
- Wrapper leases, routes, and sync locks are already scoped by canonical Herdr socket. Route state remains dedicated to the fixed `zerdr` wrapper session and does not require a schema change.
- Setup already owns manifest materialization, plugin linking, five Zed tasks, rollback, and printed keymap guidance without editing user keymaps.
- Doctor currently reports absent wrapper/session as warnings instructing bare `zerdr`, even though no-wrapper plugin-only operation will be valid.
- The active `docs/plans/2026-08-19-auto-terminal-routing.md` excludes ordinary-client synchronization and keeps bindings fixed to `zerdr`. This plan supersedes only those statements; its wrapper routing and safety contracts remain in force.

### Assumptions

- Internal helper names and type decomposition may follow existing module style as long as the contracts above remain unchanged.
- The Herdr keybinding example may be stored as a new embedded asset or a short setup constant; this does not change setup output content or ownership behavior.

## File Structure

- Modify: `assets/herdr/herdr-plugin.toml.in` — declare the workspace action alongside the existing event.
- Create: `assets/herdr/keymap.example.toml` — suggested configurable `prefix+z` action binding if implementation uses an embedded asset.
- Modify: `src/cli.rs` — add `--session` to bind/unbind and the hidden action entrypoint.
- Modify: `src/lib.rs` — dispatch session-aware binding commands and the hidden action while preserving the remote gate.
- Modify: `src/herdr.rs` — add explicit named-session/action-session targeting and session-name resolution from socket.
- Modify: `src/state.rs` — add binding schema v2, v1 read compatibility, session-scoped operations, and locked migration.
- Modify: `src/sync.rs` — separate captured-workspace one-shot routing from focused-workspace follow routing; make binding mutation wrapper-optional.
- Modify: `src/zed.rs` — add plain path opening while retaining exact existing/add process behavior.
- Modify: `src/setup.rs` — materialize/inspect the action, split launcher versus full compatibility checks, and print the Herdr keybinding example.
- Modify: `src/doctor.rs` — validate action/binding v2 state and report plugin-only operation as healthy.
- Modify: `README.md` — document the two runtime workflows, keybinding setup, binding session option, and limitations.
- Modify: `docs/plans/2026-08-19-auto-terminal-routing.md` — add a concise supersession pointer for the ordinary-client and fixed-session binding statements without rewriting its historical implementation record.
- Modify: `tests/support/mod.rs` — fake arbitrary named sessions, action context calls, plugin action metadata, and exact plain Zed invocations.
- Modify: `tests/cli_contract.rs` — public `--session`, hidden command, remote, and help contracts.
- Modify: `tests/state_and_bindings.rs` — binding schema migration, isolation, corruption, canonicalization, and concurrency.
- Modify: `tests/sync_flow.rs` — one-shot route selection, target stability, error notification, and wrapper-optional bind/unbind behavior.
- Modify: `tests/setup_and_doctor.rs` — manifest action, upgrade compatibility, setup output/rollback, and plugin-only diagnostics.

## Testing Decisions

- **Test seam:** Continue invoking the compiled binary through `assert_cmd` and isolated `TestEnv` fake Herdr/Zed executables. Use `BindingStore` directly for persisted-state and concurrency contracts.
- **Action behavior:** Invoke the hidden command only with injected plugin environment. Assert exact Herdr/Zed logs, action-target identity, foreground-restoration absence, state bytes, notification count, and exit status.
- **Authority behavior:** Use real `SyncGuard`, `LeaseSet`, and `RouteStore` fixtures. Block an action on the socket lock, change Herdr focus, then release it to prove the captured action target is retained. Test wrapper admission/fallback decisions without sleeps as correctness synchronization.
- **Migration behavior:** Start from exact schema-v1 bytes, exercise reads and concurrent mutators, and inspect schema-v2 JSON only at the documented persisted contract. Preserve existing invalid-state byte-for-byte assertions.
- **Setup behavior:** Assert generated TOML semantics and exact executable/hidden command while retaining idempotence, rollback, ownership, and no-config-edit tests.
- **Prior art:** Extend `automatic_event_without_live_lease_is_a_successful_noop`, route-corruption helpers, concurrent binding updates, setup idempotence, and doctor route fixtures rather than creating parallel harnesses.
- **Avoid:** Real Herdr/Zed configuration mutation in automated tests, tests against private helper names, polling sleeps for lock correctness, action tests that query current focus instead of using context, or assertions that plain `zed TARGET` selects a particular window beyond Zed's own setting.

## Progress

- [x] Task 1: Deliver session-scoped binding state and wrapper-optional binding commands.
- [ ] Task 2: Deliver the captured-workspace one-shot action and route arbitration.
- [ ] Task 3: Install and diagnose the complete plugin action without breaking wrapper upgrades.
- [ ] Task 4: Document both workflows and complete repository-wide validation.

Implementation-time minor file changes or internal differences must be reflected in the relevant task. Ask the user before changing requirements, Out of Scope, persisted schemas, or public contracts.

## Tasks

### Task 1: Session-Scoped Bindings and Wrapper-Optional Mutation

**Covers:** R5, R10-R12, R15, R17, D4-D6

**Objective:** Persist explicit bindings independently per Herdr session and let users repair the selected workspace mapping with or without a live wrapper.

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/lib.rs`
- Modify: `src/herdr.rs`
- Modify: `src/state.rs`
- Modify: `src/sync.rs`
- Modify: `src/doctor.rs` — adapt binding iteration to the new state shape; final presentation is completed in Task 3.
- Modify: `README.md` — keep the public bind/unbind syntax and live-wrapper requirement accurate from this task onward.
- Modify: `tests/cli_contract.rs`
- Modify: `tests/state_and_bindings.rs`
- Modify: `tests/sync_flow.rs`
- Modify: `tests/support/mod.rs`

**Dependencies:** Existing binding lock/atomic-write implementation, Herdr session listing, per-socket sync locks, and canonical Git-root resolver.

**Implementation notes:**
- Start with failing schema, session-selection, and no-wrapper command tests.
- Keep route and lease schemas unchanged. Only binding state advances to schema v2.
- Represent v2 as the documented nested session/workspace maps. All lookup and mutation APIs receive session name explicitly.
- Parse v1 and v2 separately so mixed or unknown shapes fail without overwrite. Pure reads normalize v1 in memory but retain original bytes.
- Every mutator acquires the binding lock, rereads the latest bytes under that lock, applies the operation, and atomically writes v2. This includes a no-op unbind when it is the first successful mutating operation against valid v1 state, because it is the migration boundary.
- Resolve `--session` through Herdr's session list and operate on that session's focused workspace. In complete pane context, use the injected socket and workspace ID directly; do not require that workspace to remain focused.
- Serialize binding mutation against wrapper routing with the target socket's `SyncGuard`. No-live state permits mutation. Exactly one live lease requires valid matching route authority before mutation; multiple or mismatched authority fails before changing binding bytes.
- After a successful bind under live authority, apply the existing route and retain no-rollback behavior if Zed fails. No-wrapper bind performs no Zed call. Unbind never routes.
- Keep `pick`, navigation, and follow-mode lookup explicitly scoped to session `zerdr`.

**Test cases:**
- schema-v1 state with two `zerdr` mappings → read returns equivalent in-memory session map and leaves bytes unchanged.
- first bind or no-op unbind against valid v1 → complete state is atomically written as schema v2 under `sessions.zerdr`.
- same workspace ID under `default` and `zerdr` → independent canonical roots and independent unbind behavior.
- unsupported, malformed, mixed v1/v2, or v1 non-`zerdr` session bytes + bind/unbind → failure and byte-for-byte preservation.
- concurrent first migration plus bindings in multiple sessions → no mapping lost and final schema is v2.
- `bind --session default PATH` outside Herdr → binds the focused workspace returned by that session; no live lease means no Zed call.
- complete pane environment without `--session` → binds the injected workspace on the socket's resolved session even if another workspace is focused.
- explicit `--session` inside a pane → option wins and targets the named session's focused workspace.
- only one of `HERDR_SOCKET_PATH`/`HERDR_WORKSPACE_ID` → actionable failure before binding mutation or Zed call.
- no option and no pane context → existing `zerdr` session behavior remains.
- no-wrapper unbind → removes only the selected session/workspace mapping and does not invoke Zed.
- valid live internal/external wrapper + bind → binding persists and route-specific sync remains exact.
- multiple leases, route/PID mismatch, or malformed live route + bind → failure before binding bytes or Zed calls change.

**Complete when:**
- Both binding schemas are safely readable and only valid mutations migrate state.
- Session targeting is deterministic and covered from pane and external-terminal contexts.
- Wrapper-free mutation succeeds without weakening live-route authority or existing bind synchronization.

**Validation:**
- Run: `cargo test --test state_and_bindings && cargo test --test cli_contract && cargo test --test sync_flow`
- Expected: schema migration/isolation/concurrency, CLI session selection, authority failures, and existing binding/routing regressions pass.

**Implementation record (2026-08-19):** Binding schema v2, v1 read/write migration, session-target selection, and wrapper-optional mutation were implemented across the listed production and integration files. `tests/herdr_wrapper.rs` required mechanical `BindingStore` API updates, and `README.md` was updated with the public syntax before commit. Task-level review found a lease-expiry race between initial bind authority and Zed routing; a second same-lock lease/route check plus deterministic regression test fixed it. The focused suites, `herdr_wrapper`, format, Clippy with warnings denied, and `git diff --check` passed; independent re-review approved the task.

### Task 2: Captured-Workspace One-Shot Zed Action

**Covers:** R2-R9, R16-R17, D1-D4, D6

**Objective:** Open the action's captured Git checkout in Zed from any local Herdr session, using same-socket live routing when valid and plain opening otherwise.

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/lib.rs`
- Modify: `src/herdr.rs`
- Modify: `src/sync.rs`
- Modify: `src/zed.rs`
- Modify: `tests/cli_contract.rs`
- Modify: `tests/sync_flow.rs`
- Modify: `tests/support/mod.rs`

**Dependencies:** Task 1 session-scoped binding lookup and context-aware Herdr process targeting; existing lease/route/sync-lock contracts.

**Implementation notes:**
- Start with failing hidden-entrypoint and one-shot integration tests.
- Validate the action marker and socket, then resolve the action session name by matching the canonical socket in `session list` before parsing the object-valued context and captured workspace ID. This ordering permits notification of context failures whenever the socket identifies a valid session.
- Resolve an existing session/workspace binding first and validate it exactly as follow mode does. If absent, canonicalize the documented context candidate without calling `bind_if_absent` or any state writer.
- Use the captured context only; do not call `focused_workspace` for action routing. A context candidate may be nested inside a checkout because canonical Git-root resolution remains authoritative.
- Acquire the target socket's `SyncGuard` before lease inspection and keep it until the selected Zed operation and any internal promotion complete.
- No live lease invokes a new plain-open adapter with exactly one path argument. Ignore stale route files in this branch; they do not establish authority.
- One live lease requires a route whose owner PID matches that lease. Internal and external command sequences follow R7. The external action bypasses `with_external_focus`, including routes persisted with terminal focus.
- Route/lease corruption never falls back to plain opening. Internal promotion and existing no-rollback semantics remain unchanged.
- Deliver action errors through the action session's notification boundary and return the original error so the hidden process exits nonzero. Notification failure augments rather than hides the primary failure, matching current event behavior.
- Retain the existing local-environment gate so remote hidden invocations fail before Herdr, Zed, locks, or state mutation.

**Test cases:**
- action in `default`, no live lease, worktree checkout context → exactly `zed TARGET`; no `--existing`, `--add`, focus-backend call, binding file, or route write.
- no binding and only `workspace_cwd` nested in a Git checkout → plain Zed receives the canonical root and binding state remains absent/unchanged.
- explicit binding plus different valid context checkout → bound root wins.
- stale/noncanonical binding plus valid context checkout → notification, nonzero result, no Zed call, and unchanged binding.
- present invalid/non-Git `worktree.checkout_path` plus valid `workspace_cwd` → notification and nonzero result without falling through to cwd or invoking Zed.
- non-Git context or missing workspace/root context with a valid resolved action session → notification and nonzero result without state mutation.
- action blocked on `SyncGuard`, followed by Herdr focus change before release → captured workspace root is opened, not the newly focused workspace.
- live internal route → exact existing/add order, no focus restoration, and successful anchor promotion.
- live external terminal-focus route → exact existing call and no capture/restore log; Zed remains the explicit foreground target.
- malformed/missing route with live lease, PID mismatch, or multiple live leases → notification, nonzero result, and no plain-open fallback.
- no live lease plus stale route file → plain Zed opening succeeds without route mutation.
- valid action Zed failure → one notification, nonzero exit, plugin-visible stderr, no implicit binding.
- malformed action context plus valid socket/session → one notification, nonzero stderr/plugin log, and no Zed/state mutation.
- missing or unresolvable action socket → nonzero stderr/plugin log and no notification or other target-session call.
- hidden command absent from help; launch-only options combined with it fail through Clap; remote marker rejects it before processes or state changes.
- existing `sync-from-herdr` no-live no-op and all follow-mode command sequences remain unchanged.

**Complete when:**
- One-shot behavior is selected only by same-socket authority and always uses the captured action workspace.
- Plain and live-route process logs exactly match the documented state machine.
- Failures are visible without silently opening another checkout or mutating binding state.

**Validation:**
- Run: `cargo test --test sync_flow && cargo test --test cli_contract`
- Expected: standalone/live-route action matrices, target-race coverage, notification behavior, remote/hidden CLI contracts, and existing follow sync pass.

### Task 3: Unified Plugin Installation, Upgrade Compatibility, and Diagnostics

**Covers:** R2, R13-R16, D1, D7

**Objective:** Materialize the action and configurable-key guidance through existing setup while recognizing plugin-only operation as a healthy installed state.

**Files:**
- Modify: `assets/herdr/herdr-plugin.toml.in`
- Create: `assets/herdr/keymap.example.toml` if the embedded-asset approach is used.
- Modify: `src/setup.rs`
- Modify: `src/doctor.rs`
- Modify: `tests/setup_and_doctor.rs`
- Modify: `tests/support/mod.rs`

**Dependencies:** Task 2 hidden entrypoint and exact action contract; current setup rollback/ownership model.

**Implementation notes:**
- Start with failing manifest, compatibility, setup-output, and no-wrapper doctor tests.
- Generate exactly one existing event and one workspace action with the setup-resolved executable. Preserve plugin ID, minimum Herdr version, platforms, and event command.
- Split follow capability from complete action capability. Launcher preflight accepts an enabled installed plugin and materialized manifest that satisfy the existing event/executable contract even if the action is absent. Do not automatically run setup during launch.
- Full installation inspection requires the exact action ID, title, workspace context, executable identity, and hidden command. Plugin list parsing should tolerate unrelated actions/events and require the zerdr entries by semantic identity rather than array order.
- Print the Herdr TOML keybinding after setup alongside existing Zed keymap guidance. Never locate or write the user's Herdr config.
- Preserve setup idempotence, plugin-link rollback, task ownership/fingerprints, five labels, and uninstall behavior.
- Doctor reports a complete action-enabled installation as available for one-shot use. No `zerdr` session or no live wrapper is a passing informational state when there are no live lease inconsistencies. Existing malformed route, multiple wrapper, stale cleanup, executable, capability, and installation failures remain strict.
- Validate all session/workspace binding roots with both identifiers in diagnostics. Legacy v1 remains valid and read-only doctor does not migrate it.

**Test cases:**
- fresh setup → generated manifest contains the exact event and action commands and setup output contains `prefix+z`, `plugin_action`, and `zerdr.open-zed`; no Herdr config file is created or changed.
- repeated setup → manifest/tasks/install state remain idempotent and keybinding guidance remains stable.
- plugin-link, task-write, or install-state failure → previous manifest/tasks/install ownership rollback remains byte-correct with the action included.
- plugin list with compatible event but no action + pre-action materialized manifest → bare wrapper launcher preflight succeeds.
- same incomplete install + doctor → reports missing action and `zerdr setup` remediation.
- pre-action event-only manifest/install + `zerdr setup` → installs the exact action while preserving owned Zed task migration, foreign/modified tasks, install ownership, and idempotence.
- manifest with a malformed zerdr action + `zerdr setup` → replaces it with the exact generated action under the existing rollback guarantees.
- complete plugin list/manifest with unrelated entry ordering → full inspection succeeds.
- action with wrong context, command, executable, ID, or disabled plugin → full inspection fails without weakening event-only launcher checks.
- no running `zerdr` session and no live leases → doctor reports healthy one-shot plugin mode rather than warning to start a wrapper.
- running `zerdr` session with no live wrapper → same healthy plugin-only report.
- one valid live wrapper → existing route details still pass; malformed/multiple authority still fails.
- schema-v2 bindings in multiple sessions and valid legacy v1 → doctor labels and validates every root without rewriting bytes.
- Zed lacking `--existing` or `--add` → doctor still fails under the unchanged baseline.

**Complete when:**
- Setup owns one unified plugin installation and only prints configurable key guidance.
- Binary-only upgrades preserve bare wrapper launch while doctor accurately reports action installation completeness.
- Plugin-only and live-wrapper diagnostic states are distinguished without weakening corruption checks.

**Validation:**
- Run: `cargo test --test setup_and_doctor && cargo test --test herdr_wrapper`
- Expected: manifest/setup rollback/idempotence, old-manifest launcher compatibility, complete-action diagnosis, plugin-only health, and live-route regressions pass.

### Task 4: Public Documentation and Final Validation

**Covers:** R1-R17, D1-D7

**Objective:** Document the two workflows and prove the complete change through repository gates and controlled end-to-end checks.

**Files:**
- Modify: `README.md`
- Modify: `docs/plans/2026-08-19-auto-terminal-routing.md`
- Modify: `docs/plans/2026-08-19-herdr-one-shot-zed-action.md` — record implementation differences and validation results, then archive only after every gate succeeds.

**Dependencies:** Tasks 1-3.

**Implementation notes:**
- Keep README user-facing: quickstart remains install/setup/bare wrapper, then explain plugin-only `Open Zed`, the manual Herdr keybinding, and `bind/unbind --session` recovery without contributor internals.
- State that bare wrapper follows focus, plugin-only action is one-shot, valid live routes are reused, wrapper routing corruption does not fall back, and no-wrapper plain opening follows Zed's `cli_default_open_behavior`.
- Retain local Git-only, Zed capability, setup, and remote restrictions.
- Add a concise pointer in the prior active plan identifying the new plan as the superseding contract for ordinary-client actions and binding session scope; do not rewrite its historical progress or wrapper E2E record.
- Use isolated tests for setup/doctor. Do not run `zerdr setup`, `zerdr uninstall`, or local `zerdr doctor` against the developer's real environment during automated validation.
- Record any minor implementation file differences in this plan. Ask before changing any decided public behavior or persisted schema.

**Test cases:**
- README command examples match actual help and manifest keybinding output.
- disposable default Herdr session with no wrapper + configured action → selected Git workspace opens through plain Zed and remains foreground.
- disposable external follow wrapper with terminal focus + action → action uses the live external route and leaves Zed foreground rather than restoring the terminal.
- disposable internal wrapper + action → target is routed through the same anchored Zed window.
- `bind --session` against disposable sessions with identical workspace IDs → mappings remain independent and one-shot actions use the matching roots.
- stale binding in a disposable session → visible notification and no alternate checkout opens.
- remote and non-Git cases remain covered automatically; no real remote E2E is required.

**Complete when:**
- Documentation exposes both workflows without implying a right-click action or automatic keymap mutation.
- Focused and full automated validation passes in the repository-required order.
- Applicable disposable-session/Zed E2E observations are recorded; unavailable GUI checks remain unchecked rather than assumed.
- The plan and actual changed-file set agree, Requirement Coverage has no gap, and no release operation occurred.

**Validation:**
- Run: `cargo test --test state_and_bindings && cargo test --test sync_flow && cargo test --test setup_and_doctor && cargo test --test cli_contract && cargo test --test herdr_wrapper`
- Expected: every changed subsystem's integration suite passes before broad gates.
- Run: `cargo fmt --all -- --check`
- Expected: no formatting diff.
- Run: `cargo clippy --all-targets --all-features -- -D warnings`
- Expected: no warning or error.
- Run: `cargo test --all-targets --all-features`
- Expected: all existing and new tests pass.
- Run: `git diff --check`
- Expected: no whitespace error.
- Run: controlled manual cases above with disposable Herdr sessions and Git roots; do not mutate the developer's normal setup.
- Expected: observed Zed commands/focus, binding isolation, and notification behavior match the contracts; if safe disposable GUI validation is unavailable, record it as unverified and do not archive.

## Requirement Coverage

| Requirement / Decision | Task(s) | Verification |
|---|---|---|
| R1, D1 | 2-4 | Existing follow regressions; unified manifest tests; README/manual wrapper checks |
| R2 | 2-3 | Hidden CLI and generated manifest action assertions |
| R3-R4, D2, D6 | 1-2 | Arbitrary-session fixtures and lock-delayed focus-change action test |
| R5, D4 | 1-2 | Binding precedence, context fallback/no-persistence, stale/non-Git failures |
| R6-R8, D3 | 2, 4 | Exact plain/internal/external logs, route corruption tests, foreground E2E |
| R9 | 2 | Notification count, nonzero status, malformed-context stderr tests |
| R10, D5 | 1, 3 | v1 read/no-rewrite, v2 mutation, corruption preservation, doctor fixtures |
| R11-R12 | 1 | Session precedence and no-wrapper/live-wrapper bind/unbind matrices |
| R13 | 3-4 | Setup output/no-config-write tests and README keybinding example |
| R14, D7 | 3 | Old-manifest launcher success plus doctor/setup remediation tests |
| R15 | 1, 3 | Multi-session binding validation and plugin-only/live authority doctor tests |
| R16 | 2-4 | Remote hidden-command test, unchanged doctor capability failures, documented baseline, full gates |
| R17 | 1-2 | Existing command regressions, hidden help, launch-option conflict tests |

## Final Validation

- [ ] `cargo test --test state_and_bindings` — Expected: binding v1/v2 compatibility, session isolation, canonicalization, invalid-byte preservation, and concurrency pass.
- [ ] `cargo test --test sync_flow` — Expected: one-shot plain/live routing, captured target, notification, binding command authority, and existing follow behavior pass.
- [ ] `cargo test --test setup_and_doctor` — Expected: unified manifest/action setup, rollback, upgrade compatibility, binding diagnosis, and plugin-only health pass.
- [ ] `cargo test --test cli_contract && cargo test --test herdr_wrapper` — Expected: public/hidden CLI and existing wrapper lifecycle contracts pass.
- [ ] `cargo fmt --all -- --check` — Expected: no formatting diff.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` — Expected: no warning or error.
- [ ] `cargo test --all-targets --all-features` — Expected: all tests pass.
- [ ] `git diff --check` — Expected: no whitespace error.
- [ ] Controlled disposable-session/Zed E2E — Expected: plugin-only plain open, live external foreground override, live internal anchored routing, session binding isolation, and stale-binding notification match the documented behavior; leave unchecked if unavailable.
- [ ] Requirement Coverage has no unmapped requirement or decision.
- [ ] The plan and actual changed-file set agree, including recorded minor implementation differences.
- [ ] No release, tag, or Homebrew tap mutation occurred.
- [ ] After every item above succeeds, move this plan without renaming to `docs/plans/archived/2026-08-19-herdr-one-shot-zed-action.md`.

## Risks and Open Questions

### Risks

- Binding migration and concurrent writes can lose mappings if legacy normalization occurs outside the exclusive lock; mutation tests must force the first-write race.
- Herdr workspace IDs such as `w0` are scoped to a session. Session-name scoping prevents live cross-session collisions but intentionally preserves mappings when a named session is restarted; stale roots remain explicit errors rather than implicit remapping.
- A wrapper may start while a one-shot action is deciding whether to use plain Zed. Holding `SyncGuard` through lease inspection and Zed execution is required to keep the decision atomic with admission.
- The action context is a snapshot while Herdr focus is mutable. Reusing focused-workspace sync helpers would open the wrong checkout under contention.
- Plain `zed TARGET` delegates window placement to Zed's `cli_default_open_behavior`; tests must assert argv rather than a window-selection guarantee.
- A keybinding can conflict with user configuration. Setup only prints the suggested binding and leaves conflict resolution to the user.

### Open Questions

- None. Public behavior, CLI surface, persisted binding schema, migration boundary, routing authority, error delivery, setup ownership, and diagnostic status were resolved before planning.
