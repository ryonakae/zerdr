# Implicit Workspace Creation for `zerdr connect` Implementation Plan

> **For implementers:** Execute tasks in order unless dependencies allow otherwise. Mark a task complete only after its validation succeeds. Reflect minor implementation differences in the relevant task. Ask the user before changing requirements, Out of Scope, or public contracts.

## Problem Statement

A terminal opened in a Git checkout that Herdr does not yet manage requires `zerdr connect --create`, while subsequent connects use plain `zerdr connect`. The extra flag makes the common first-connect path needlessly different. The same flag also starts a stopped named session headless, mixing workspace preparation with session lifecycle and producing a session without zerdr's route or focus-sync wrapper.

## Goal

Make `zerdr connect` the single manual command for attaching a Zed terminal thread: reuse an existing matching workspace when possible and create or register one when missing. Remove `--create` and its headless named-session startup behavior. A stopped session remains the responsibility of `zerdr start`.

## Out of Scope

- Backward compatibility, aliases, or a deprecation period for `--create`.
- Starting any Herdr session from `zerdr connect`, including named sessions.
- Changing explicit `TARGET` resolution, pane selection order, lease ownership, title/bell behavior, detach/attach behavior, or focus behavior.
- Changing auto-mode enablement or best-effort failure handling.
- Creating or deleting Git worktrees; zerdr continues only to register an existing linked worktree with `herdr worktree open`.
- Changing persisted binding, route, lease, or thread-memory schemas.

## Requirements and Decisions

### Requirements

- **R1:** Manual `zerdr connect` without `TARGET` resolves the current Git root and uses the existing workspace-selection order. If no workspace matches, it creates and binds a Herdr workspace before attaching.
- **R2:** In a primary checkout, create-on-miss continues to call `herdr workspace create --cwd <root> --label <dirname> --no-focus`, bind the returned workspace, and attach to its root pane.
- **R3:** In a linked worktree, create-on-miss continues to call `herdr worktree open --cwd <primary-checkout> --path <root> --no-focus`, bind the returned workspace, and attach to its root pane. Failure is surfaced to a manual caller with no plain-workspace fallback and no binding write.
- **R4:** `--create` is removed from the public CLI. Passing it is a clap unknown-argument error; no compatibility alias or hidden form remains.
- **R5:** `zerdr connect TARGET` keeps direct pane/agent attachment and does not resolve or create a workspace. `--kind` remains incompatible with `TARGET`.
- **R6:** `zerdr connect --auto` keeps the same create-on-miss path while auto mode is enabled and continues to swallow attach/setup failures so the Zed terminal remains a local shell. It never starts a session server.
- **R7:** If the selected session is not running, manual `connect` exits nonzero without spawning Herdr. The error points to `zerdr start` for the default session and `zerdr start --session NAME` for a named session. Auto mode keeps its silent plain-shell fallback.
- **R8:** Remove code and test-fixture support that exists only for `connect --create` headless session startup: the detached server adapter, readiness/timeout path, session-start lock helper, fake server handler, and file-backed session-list controls used by those tests.
- **R9:** Existing workspace matching, free-agent → remembered-shell → new-tab selection, per-pane leases, and per-session resolve serialization remain unchanged.
- **R10:** User and maintainer documentation describe bare `connect` creation and `start`-owned session lifecycle without mentioning `--create` as a supported command.

### Implementation Decisions

- **D1:** Workspace creation is unconditional on a workspace miss for both manual and enabled-auto bare connects. Manual and auto differ only in whether failures are returned or swallowed.
- **D2:** Session lifecycle and workspace lifecycle are separated: `connect` requires an already-running session; `start` owns launching Herdr with zerdr routing and focus sync.
- **D3:** Remove the `create: bool` plumbing rather than defaulting it to `true`. This keeps the runtime contract aligned with the CLI and prevents the deleted headless-start capability from remaining reachable internally.
- **D4:** Remove headless-start-only implementation and fake support instead of preserving unused compatibility code.
- **D5:** This is an intentional breaking CLI change with no migration period, per the user's decision.

### Contracts

Public CLI:

```text
zerdr connect [TARGET] [--kind KIND] [--session NAME]
```

- Hidden `--auto` remains accepted and conflicts with `TARGET` and `--kind`.
- `--create` is not accepted.
- Missing manual session errors identify the session-start command. Exact prose may follow existing error style, but command guidance is fixed:
  - default: `zerdr start`
  - named: `zerdr start --session NAME`
- Manual workspace-create/register failures remain nonzero user-visible errors; auto-mode failures remain silent successful fallback.

## Current Context

### Confirmed

- `src/cli.rs` currently defines `create: bool`, conflicts it with `TARGET`, and includes it in hidden `--auto` conflicts.
- `src/lib.rs` passes `create` into `thread::run`; `src/thread.rs::run_with_mode` uses the same boolean for both workspace creation and stopped named-session startup.
- `src/thread.rs::resolve_or_create` already contains the desired primary-checkout and linked-worktree create paths, binding write, root-pane lease, attach, and status behavior. Removing the refusal branch makes this the normal bare-connect miss path.
- The resolve sequence is already serialized per session socket before workspace listing, preventing concurrent bare connects from racing through an unmatched workspace without coordination.
- Enabled auto mode already passes through the same create path and swallows remaining errors in `run_auto`.
- Headless startup is isolated to `Herdr::spawn_server_detached_for`, `ThreadLeaseSet::session_start_lock_path`, `resolve_session_socket`, and dedicated `thread_flow`/fake-Herdr support.
- `README.md`, `AGENTS.md`, and released changelog text currently describe `--create` and headless named-session startup.
- The repository requires behavior changes to be integration-tested first, followed by fmt, clippy, and the full test suite.

### Assumptions

None.

## File Structure

- Modify: `src/cli.rs` — remove the `--create` argument and update conflict declarations.
- Modify: `src/lib.rs` — remove `create` dispatch plumbing.
- Modify: `src/thread.rs` — require an already-running session, make workspace create/register unconditional on miss, remove headless-start readiness logic and obsolete imports/status output.
- Modify: `src/herdr.rs` — remove the headless detached-server adapter.
- Modify: `src/state.rs` — remove the session-name start-lock path helper.
- Modify: `tests/cli_contract.rs` — pin the new help and rejection contract.
- Modify: `tests/thread_flow.rs` — replace explicit-create/refusal/headless-start cases with bare-connect create/register and stopped-session cases.
- Modify: `tests/support/mod.rs` — remove fake-Herdr behavior and environment controls used only by headless startup tests.
- Modify: `README.md` — document implicit workspace creation and `start`-owned session startup.
- Modify: `AGENTS.md` — update the `src/thread.rs` repository-map description.
- Modify: `CHANGELOG.md` — add an unreleased breaking-change entry without rewriting historical release entries.

## Testing Decisions

- **Test seam:** drive the compiled binary through `assert_cmd` and `TestEnv` fake Herdr executables, matching existing CLI and thread-flow coverage.
- **Behavior:** assert public help/rejection, Herdr argv, bindings, attach target, exit status, and stopped-session no-spawn behavior.
- **Prior art:** adapt the existing `create_makes_the_workspace_binds_it_and_starts_an_agent`, linked-worktree create/failure tests, bare no-match refusal tests, and named-session startup tests in `tests/thread_flow.rs`; adapt the current connect option/conflict table in `tests/cli_contract.rs`.
- **Avoid:** unit tests for private boolean plumbing, exact full clap help snapshots, real Herdr/Zed during automated validation, fixed sleeps, or fallback from failed worktree registration.

## Progress

- [x] Task 1: Replace the `--create` contract with bare-connect workspace preparation
- [x] Task 2: Remove headless connect startup support and align documentation

Implementation-time minor file changes or internal differences belong in the relevant task. Ask before changing requirements, Out of Scope, or public contracts.

## Tasks

### Task 1: Replace the `--create` contract with bare-connect workspace preparation

**Covers:** R1, R2, R3, R4, R5, R6, R7, R9, D1, D2, D3, D5

**Objective:** A bare manual connect creates or registers a missing workspace and attaches, while `--create` is rejected and stopped sessions point to `start` without spawning anything.

**Files:**
- Modify: `tests/cli_contract.rs`, `tests/thread_flow.rs`
- Modify: `src/cli.rs`, `src/lib.rs`, `src/thread.rs`

**Dependencies:** none

**Implementation notes:**
- Follow TDD: first change integration expectations so bare connect in unmatched primary and linked checkouts must create/register, and so `--create` is absent/rejected; verify the focused tests fail before implementation.
- Remove `create` from `Command::Connect`, `thread::run`, and `run_with_mode`. Hidden `--auto` conflicts only with `target` and `kind`.
- Simplify session resolution to return an existing socket or an actionable error. Do not retain a dormant start flag, readiness result, or "started Herdr session" status path.
- Remove the `!create` refusal branch from workspace resolution while preserving the surrounding resolve lock, match order, linked-worktree detection, no-fallback rule, binding write, lease, and `Attachment::NewWorkspace` status.
- Keep explicit `TARGET` before Git-root/workspace resolution so it never creates a workspace.
- Manual errors propagate from `thread::run`; `run_auto` continues to swallow `run_with_mode` errors.

**Test cases:**
- `zerdr connect --help` → shows `TARGET` and `--kind`; does not show `--create` or hidden `--auto`.
- `zerdr connect --create` → clap failure (exit 2), mentions an unexpected/unknown argument, and does not invoke Herdr.
- `zerdr connect TARGET --kind pi` → remains a clap conflict; direct `zerdr connect TARGET` still attaches without workspace creation.
- Bare manual connect in an unmatched primary checkout → invokes `workspace create` with the existing cwd/label/no-focus arguments, writes the binding, attaches to the returned root pane, and reports the created workspace.
- Bare manual connect in an unmatched linked worktree → invokes `worktree open` with the existing primary/root arguments, does not invoke `workspace create`, writes the binding, and attaches.
- Linked-worktree registration failure under manual bare connect → exits nonzero with Herdr's error, writes no binding, and does not fall back to `workspace create`.
- Enabled `connect --auto` in an unmatched checkout/worktree → retains the same successful create/register behavior and best-effort contract.
- Bare connect against a stopped default session → exits nonzero, mentions `zerdr start`, and logs no server invocation.
- Bare connect against a stopped named session → exits nonzero, mentions `zerdr start --session NAME`, and logs no server invocation.
- Bare connect against a running named session → follows the normal workspace match/create/attach flow without spawning a server.

**Complete when:**
- Focused tests demonstrate the new CLI and runtime contract.
- Existing matching, lease, remembered-pane, new-tab, auto, detach/attach, and title/bell tests remain green.
- No runtime `create` boolean or workspace-miss refusal remains.

**Validation:**
- Red: `cargo test --test cli_contract && cargo test --test thread_flow`
- Expected before implementation: failures specifically reflect the removed option, new bare create-on-miss behavior, and stopped-session guidance.
- Green: `cargo test --test cli_contract && cargo test --test thread_flow`
- Expected after implementation: all tests pass.

**Result (2026-08-27):** Implemented through the public binary seam. `cli_contract` passes 24 tests and `thread_flow` passes 57 tests. Bare connects now create/register on miss; explicit targets, matching order, leases, auto fallback, and stopped-session no-spawn behavior remain covered. The existing "bound elsewhere" case now verifies that zerdr preserves the foreign binding and creates a distinct workspace for the current checkout.

### Task 2: Remove headless connect startup support and align documentation

**Covers:** R8, R10, D2, D4, D5

**Objective:** No implementation, fake, or documentation implies that `connect` can start a session; user-facing docs consistently describe implicit workspace creation and `zerdr start` session ownership.

**Files:**
- Modify: `src/herdr.rs`, `src/state.rs`
- Modify: `tests/support/mod.rs`, `tests/thread_flow.rs`
- Modify: `README.md`, `AGENTS.md`, `CHANGELOG.md`

**Dependencies:** Task 1 establishes the replacement contract.

**Implementation notes:**
- Delete `Herdr::spawn_server_detached_for` and `ThreadLeaseSet::session_start_lock_path` after their call sites are gone. Preserve `ManagedChild`, process-group handling used by wrappers/attachments, and unrelated lock helpers.
- Delete fake-Herdr `--session NAME server` handling and file-backed session-list controls only after confirming no remaining test uses `ZERDR_TEST_SESSIONS_FILE`, `ZERDR_TEST_SESSIONS_STARTED_JSON`, or `ZERDR_TEST_SERVER_SLEEP`.
- Remove obsolete headless readiness, timeout, started-status, and running-with-create tests rather than translating them into tests for nonexistent behavior. Keep/replace the stopped-session and running-named-session coverage listed in Task 1.
- README terminal-thread prose should say manual and auto connects create/register a missing workspace, while only auto suppresses failures. The named-session section should say `connect --session NAME` requires a running session and directs startup to `zerdr start --session NAME`.
- Update the command table and session-lifetime note. Preserve the distinction that zerdr registers but never creates/removes Git worktrees.
- Add an `Unreleased` breaking-change entry to `CHANGELOG.md`; do not alter v0.3.0's historical account of what that release shipped.
- Update `AGENTS.md` only where its current repository map describes `--create` and headless startup.

**Test cases:**
- Source sweep finds no live-code, test, README, or AGENTS references to `--create`, headless connect startup, its server adapter, start lock, or dedicated fake environment variables. Historical plans and released changelog entries are excluded from this sweep.
- `cargo run --locked -- connect --help` matches the documented command table.

**Complete when:**
- Headless-connect-only code and test support are removed without deleting shared process/lease functionality.
- README, AGENTS, and the unreleased changelog entry match the implemented behavior.
- Focused and full validation pass.

**Validation:**
- Run: `rg -n --glob '!docs/plans/**' --glob '!CHANGELOG.md' --glob '!tests/cli_contract.rs' -- '--create|spawn_server_detached_for|session_start_lock_path|ZERDR_TEST_SESSIONS_FILE|ZERDR_TEST_SESSIONS_STARTED_JSON|ZERDR_TEST_SERVER_SLEEP|started Herdr session|headless' src tests README.md AGENTS.md`
- Expected: no matches. `tests/cli_contract.rs` is excluded because it intentionally pins `--create` rejection.
- Run: `cargo run --locked -- connect --help`
- Expected: help shows `TARGET` and `--kind`, with no `--create`.

**Result (2026-08-27):** Removed the detached server adapter, session-start lock helper, readiness/timeout branch, dedicated fake server/session-list controls, and obsolete tests. README, AGENTS.md, and an Unreleased changelog entry now describe implicit workspace creation and `start`-owned session startup. The stale-reference sweep is clean outside the intentional CLI rejection test, and `cargo run --locked -- connect --help` matches the documented surface.

## Requirement Coverage

| Requirement / Decision | Task | Verification |
|---|---|---|
| R1–R3 implicit primary/worktree creation | Task 1 | `thread_flow` bare-connect create/register/surface-failure cases |
| R4 remove `--create` | Task 1 | `cli_contract` help and unknown-argument cases |
| R5 explicit target unchanged | Task 1 | `cli_contract` conflict + `thread_flow` direct attach case |
| R6 auto behavior unchanged | Task 1 | existing and adapted auto create/fallback cases |
| R7 stopped sessions use `start` | Task 1 | default/named stopped-session cases assert guidance and no server log |
| R8 remove headless-only support | Task 2 | source sweep + compiler/full test suite |
| R9 preserve matching/lease ordering | Task 1 | existing `thread_flow` matching, concurrency, memory, and lease cases |
| R10 documentation alignment | Task 2 | source sweep + help/README comparison |
| D1, D3 implicit path without boolean | Task 1 | signatures/code review + create-on-miss cases |
| D2 session/workspace lifecycle split | Tasks 1–2 | stopped-session cases + docs |
| D4 dead-code removal | Task 2 | source sweep + compiler |
| D5 breaking change | Tasks 1–2 | CLI rejection + Unreleased changelog entry |

## Final Validation

- [ ] `cargo test --test cli_contract` — Expected: all CLI contract tests pass.
- [ ] `cargo test --test thread_flow` — Expected: all connect flows pass, including implicit primary/worktree creation and stopped-session no-spawn cases.
- [ ] `cargo fmt --all -- --check` — Expected: no formatting diff.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` — Expected: no warnings.
- [ ] `cargo test --all-targets --all-features` — Expected: all tests pass.
- [ ] `rg -n --glob '!docs/plans/**' --glob '!CHANGELOG.md' --glob '!tests/cli_contract.rs' -- '--create|spawn_server_detached_for|session_start_lock_path|ZERDR_TEST_SESSIONS_FILE|ZERDR_TEST_SESSIONS_STARTED_JSON|ZERDR_TEST_SERVER_SLEEP|started Herdr session|headless' src tests README.md AGENTS.md` — Expected: no matches; `tests/cli_contract.rs` intentionally pins rejection of the removed option.
- [ ] `cargo run --locked -- connect --help` — Expected: public help matches the new contract and README.
- [ ] Manual real-environment validation: N/A by default because automated integration tests cover the Herdr argv and repository safety guidance forbids environment-facing setup operations. If a developer chooses to smoke-test later, use an isolated already-running session and disposable checkout; do not start or mutate the normal development session.
- [ ] Requirement Coverage has no unaddressed rows.
- [ ] The plan and actual changes are consistent.
- [ ] After all checks succeed, move this file unchanged to `docs/plans/archived/`.

## Risks and Open Questions

- Risk: bare manual connect now mutates Herdr workspace state when run from the wrong Git checkout. This is accepted because `connect` is an explicit user action and the desired common-case contract is create-on-miss.
- Risk: linked-worktree registration can fail when the parent checkout is not usable by Herdr. Manual connect must surface that error and must not hide it with a plain-workspace fallback.
- Risk: removing headless startup means a stopped named session requires a separate `zerdr start --session NAME` invocation. This is intentional so the resulting session has zerdr routing and focus sync.
- Open questions: none.
