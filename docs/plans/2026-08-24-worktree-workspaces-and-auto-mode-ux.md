# Worktree-Backed Workspace Registration and Auto Mode UX Implementation Plan

> **For implementers:** Execute tasks in order unless dependencies allow otherwise. Mark a task complete only after its validation succeeds. Reflect minor implementation differences in the relevant task. Ask the user before changing requirements, Out of Scope, or public contracts.

## Problem Statement

Git worktrees created outside Herdr (Worktrunk's `wt`, plain `git worktree add`, other tools) have no matching Herdr workspace, so `zerdr connect` in such a checkout dead-ends, and `zerdr connect --create` registers a plain workspace that Herdr does not recognize as a worktree (`herdr worktree list`/`remove` cannot see or clean it). After a worktree is deleted externally, zerdr bindings keep pointing at the missing path with no guidance. Separately, `zerdr connect --auto` exits silently when auto mode is disabled — indistinguishable from a bug — and `zerdr setup auto on|off` reads worse than `enable|disable`.

Investigated and confirmed during requirements: worktree-backed workspaces that Herdr already manages (created via `herdr worktree create/open`) match and attach correctly with the current binary; the reported failure came from an older binary plus an unregistered worktree. The gap is registration of externally created worktrees, cleanup guidance, and the auto-mode UX.

## Goal

- `zerdr connect --create` (and the auto-mode create path) run inside a linked Git worktree registers the checkout with `herdr worktree open`, producing a worktree-backed workspace that converges with the `herdr worktree` model regardless of which tool created the worktree.
- `zerdr setup doctor` guides the user to `herdr worktree remove` when a binding points at a deleted checkout.
- `zerdr connect --auto` explains itself when auto mode is disabled.
- The auto subcommand reads `zerdr setup auto enable|disable`.

## Out of Scope

- zerdr never creates or removes Git worktrees; creation stays with `wt`, `git worktree`, `herdr worktree create`, etc.
- No automatic pruning of bindings whose paths are missing (transient mount/network absence must not destroy state).
- No fallback from `herdr worktree open` to plain `workspace create` (a silent fallback would recreate the unregistered-worktree inconsistency this plan removes).
- No change to matching of workspaces Herdr already manages (confirmed working; covered by existing tests).
- No `on`/`off` aliases after the rename (pre-release tool, clean break, matching the `a377086` restructure).
- Other silent auto-mode outcomes (outside a Git checkout, Herdr not running) stay silent; only the mode-disabled case gains a notice.

## Requirements and Decisions

### Requirements

- **R1:** When the resolved Git root is a linked worktree, the workspace-creating paths of `zerdr connect` (explicit `--create` and auto mode's create-on-miss) call `herdr worktree open --path <root> --no-focus` instead of `workspace create`, then bind, lease, and attach exactly as the plain-create path does today.
- **R2:** When the resolved Git root is a primary (non-linked) checkout, the create path is unchanged: `workspace create --cwd <root> --label <dirname> --no-focus`.
- **R3:** If `herdr worktree open` fails (old Herdr, Git refusal, any nonzero exit), `zerdr connect --create` fails with Herdr's error surfaced; no workspace is created another way and no binding is written. The auto path keeps its existing best-effort contract (failure leaves a plain local shell, silently).
- **R4:** Bare `zerdr connect` (no `--create`) still refuses to create. When the unmatched root is a linked worktree, the error says `--create` will open this worktree as a Herdr workspace; the plain-checkout message keeps its current meaning.
- **R5:** When `zerdr setup doctor` finds a binding whose path no longer exists, the failure message additionally guides: if the path was a removed Git worktree, close it with `herdr worktree remove --workspace <id>`; otherwise rebind with `zerdr workspace bind`. No state is modified.
- **R6:** `zerdr setup auto` accepts `enable` and `disable`; `on` and `off` are rejected by clap with the possible values listed. Every user-facing string that says `zerdr setup auto on|off` (setup install output, doctor hints, help text) says `enable|disable` instead. Flag-file semantics are unchanged (`disable` still leaves Zed settings untouched).
- **R7:** `zerdr connect --auto` with auto mode disabled prints one stdout line — auto mode is disabled; run `zerdr connect` to attach this thread to a Herdr pane, or run `zerdr setup auto enable` to attach new threads automatically — and exits 0 without contacting Herdr.
- **R8:** README describes the worktree registration behavior and uses the renamed subcommand.

### Implementation Decisions

- **D1:** Linked-worktree detection is tool-agnostic: compare `git rev-parse --git-dir` with `--git-common-dir` (absolute paths; canonicalize before comparing rather than relying on `--path-format=absolute`, which needs Git ≥ 2.31). They differ exactly for linked worktrees. Helper lives in `state.rs` beside `canonical_git_root`.
- **D2:** `worktree open` is not passed `--label`; Herdr derives the label (branch-based, as observed for live worktree workspaces w16/w17). The status line shows the label from the response when present, falling back to the current directory-name label.
- **D3:** Doctor's guidance is message-only. Once the path is gone, zerdr cannot verify whether it was a worktree, so the message offers both the worktree-removal and rebind remedies instead of querying live sessions.
- **D4:** The disabled-mode notice prints on every new thread while `terminal_init_command` is installed and the mode is disabled. Accepted noise: it is exactly the state the user found confusing, and `zerdr setup uninstall` remains the way to remove the init command.

### Contracts

- `state.rs`: `pub fn is_linked_worktree(root: &Path) -> Result<bool>` — `root` must be a Git checkout root; errors mirror `canonical_git_root` failures.
- `herdr.rs`: `pub fn worktree_open_for(&self, session_name: &str, root: &Path) -> Result<CreatedWorkspace>` — runs `worktree open --path <root> --no-focus` through the session wrapper, parses `.result.workspace.workspace_id` and `.result.root_pane.pane_id` (same shape as `workspace create`; Herdr docs: "Worktrees are normal Herdr workspaces"), plus `.result.workspace.label` when present. `CreatedWorkspace` may grow an optional label field for D2.
- CLI: `zerdr setup auto <enable|disable>` (ValueEnum `Enable`/`Disable`); persisted state files unchanged.
- Exact user-facing strings (adjust grammar during implementation, keep the command references):
  - R4 worktree no-match: `no Herdr workspace matches <root>; run \`zerdr connect --create\` to open this Git worktree as a Herdr workspace, or bind one with \`zerdr workspace bind\``
  - R7 notice: `zerdr: auto mode is disabled; run \`zerdr connect\` to attach this thread to a Herdr pane, or run \`zerdr setup auto enable\` to attach new threads automatically`

## Current Context

### Confirmed

- `canonical_git_root` (`src/state.rs:346`) returns the worktree's own toplevel inside a linked worktree, so a worktree is already a distinct root; matching against Herdr-managed worktree workspaces works via `worktree.checkout_path` (`src/thread.rs:414`, `src/herdr.rs:657`) and is covered by `tests/thread_flow.rs`.
- The create branch to change is in `resolve_or_create` (`src/thread.rs:236-260`); auto mode reuses it (create-on-miss), so R1 covers both entry points with one switch.
- `herdr worktree open (--path PATH | --branch NAME) [--label TEXT] [--focus|--no-focus]` exists in Herdr 0.8.2; `workspace create`'s JSON exposes `.result.workspace.workspace_id` and `.result.root_pane.pane_id` (Herdr CLI reference).
- Doctor already fails on missing binding paths (`src/doctor.rs:118-147`); the auto-mode hints to update are at `src/doctor.rs:328-356`; setup install's hint is at `src/setup.rs:247`; the auto flag gate is `src/thread.rs:37-44`.
- `AutoState { On, Off }` is defined at `src/cli.rs:93-97` and dispatched in `src/lib.rs:92-95`.
- Test seams: fake `herdr` in `tests/support/mod.rs` matches argv per command (e.g. `workspace create` at line 199) and is driven by env vars like `ZERDR_TEST_WORKSPACE_CREATE_JSON`; `cli_contract.rs` asserts usage and the silent disabled auto path (`connect_auto_is_silent_when_disabled_without_touching_herdr`, line 347); `setup_and_doctor.rs` covers doctor output.

### Assumptions

- `herdr worktree open`'s JSON response mirrors `workspace create` (`.result.workspace`, `.result.root_pane`). Documented model supports this; verified against real Herdr in the manual Final Validation step. A mismatch only changes the parsing pointers inside `worktree_open_for`.

## File Structure

- Modify: `src/state.rs` — add `is_linked_worktree` beside `canonical_git_root`.
- Modify: `src/herdr.rs` — add `worktree_open_for`; optional label on `CreatedWorkspace`.
- Modify: `src/thread.rs` — create-branch switch, worktree-aware no-match message, disabled-auto notice in `run_auto`.
- Modify: `src/cli.rs`, `src/lib.rs` — `AutoState` rename and help text.
- Modify: `src/setup.rs`, `src/doctor.rs` — renamed hints; doctor's missing-binding guidance.
- Modify: `tests/support/mod.rs` — fake `herdr` handler for `worktree open` driven by `ZERDR_TEST_WORKTREE_OPEN_JSON` (keep the shared fake free of per-invocation setup).
- Test: `tests/thread_flow.rs` — worktree create/failure/no-match/auto cases.
- Test: `tests/cli_contract.rs` — enable/disable contract; disabled-auto notice.
- Test: `tests/setup_and_doctor.rs` — doctor guidance and renamed hints.
- Modify: `README.md` — worktree registration paragraph; `enable`/`disable` wording.

## Testing Decisions

- **Test seam:** the zerdr binary against the fake `herdr`/`zed` executables via `TestEnv`, asserting the logged Herdr argv, exit codes, stdout, and persisted bindings — same seam as the existing create tests.
- **Behavior:** fixtures gain a linked worktree made with real `git worktree add` from the fixture repo (both CI platforms have Git); tests drive `connect --create`, bare `connect`, and `connect --auto` from inside it.
- **Prior art:** `auto_without_a_matching_workspace_creates_and_binds_one` (`tests/thread_flow.rs:405`) for create-path assertions; `connect_auto_is_silent_when_disabled_without_touching_herdr` for the notice; usage assertions in `cli_contract.rs:50,143`.
- **Avoid:** asserting on Herdr JSON internals beyond what the adapter parses; per-invocation setup in the shared fake `herdr` (wrapper tests are time-budgeted).

## Progress

- [ ] Task 1: Register linked worktrees via `herdr worktree open`
- [ ] Task 2: Worktree-aware no-match error
- [ ] Task 3: Doctor guidance for deleted worktree checkouts
- [ ] Task 4: Rename `setup auto on|off` to `enable|disable`
- [ ] Task 5: Disabled-auto notice
- [ ] Task 6: README update

## Tasks

### Task 1: Register linked worktrees via `herdr worktree open`

**Covers:** R1, R2, R3, D1, D2

**Objective:** `zerdr connect --create` (and auto's create-on-miss) inside a linked worktree registers a worktree-backed workspace; plain checkouts keep the existing path; `worktree open` failure aborts without fallback.

**Files:**
- Modify: `src/state.rs`, `src/herdr.rs`, `src/thread.rs`
- Modify: `tests/support/mod.rs`
- Test: `tests/thread_flow.rs`

**Dependencies:** none

**Implementation notes:**
- Detection per D1; call it only on the create branch so matched attaches pay no extra git invocation.
- `worktree_open_for` follows the `workspace_create_for` pattern (session wrapper, argv building, error mapping); per D2 no `--label` is passed and the response label feeds `Attachment::NewWorkspace`.
- Keep the created-workspace flow after the call identical (bind_if_absent, start_and_lease, remember_pane) so R3's "no binding on failure" falls out of the early `?` return.
- Fake `herdr`: add a `worktree open` branch mirroring the `workspace create` one (log argv, emit `ZERDR_TEST_WORKTREE_OPEN_JSON`); default the env var in `TestEnv` so unrelated tests stay untouched.

**Test cases:**
- `connect --create` from inside a linked worktree of the fixture repo, no matching workspace → log contains `worktree open --path <worktree-root> --no-focus` and no `workspace create`; binding maps the new workspace id to the worktree root; status line reports the created workspace with the response label; attach reaches `terminal attach`.
- `connect --create` from the plain fixture checkout → unchanged `workspace create --cwd ... --label <dirname> --no-focus` (existing tests stay green).
- `connect --auto` (flag on) from inside a linked worktree with no matching workspace → same `worktree open` path as explicit create.
- Fake `worktree open` exits nonzero with a stderr message → `connect --create` exits nonzero, surfaces the message, log contains no `workspace create`, bindings file has no new entry.

**Complete when:**
- All four cases pass; existing thread_flow tests are unmodified except where they assert full usage strings.
- Validation succeeds.

**Validation:**
- Run: `cargo test --test thread_flow`
- Expected: all tests pass, including the four new cases.

### Task 2: Worktree-aware no-match error

**Covers:** R4

**Objective:** Bare `zerdr connect` in an unmatched linked worktree explains that `--create` opens the worktree as a Herdr workspace.

**Files:**
- Modify: `src/thread.rs`
- Test: `tests/thread_flow.rs`

**Dependencies:** Task 1 (`is_linked_worktree`).

**Implementation notes:**
- Branch only the message at `src/thread.rs:238-241`; behavior (refuse to create, nonzero exit) is unchanged. Use the R4 string from Contracts.

**Test cases:**
- Bare `connect` from inside a linked worktree, no match → nonzero exit; stderr contains "open this Git worktree as a Herdr workspace" and `zerdr connect --create`.
- Bare `connect` from a plain unmatched checkout → existing message unchanged.

**Complete when:** both cases pass under the Task 1 fixtures.

**Validation:**
- Run: `cargo test --test thread_flow`
- Expected: pass.

### Task 3: Doctor guidance for deleted worktree checkouts

**Covers:** R5, D3

**Objective:** A binding whose path is gone yields a doctor failure that names both remedies: `herdr worktree remove --workspace <id>` for a removed worktree, `zerdr workspace bind` otherwise.

**Files:**
- Modify: `src/doctor.rs`
- Test: `tests/setup_and_doctor.rs`

**Dependencies:** none

**Implementation notes:**
- Extend the `Err` arm of the binding check (`src/doctor.rs:133-138`) message; no live Herdr queries, no state mutation (D3). Interpolate the session and workspace id already in scope.

**Test cases:**
- Bindings file with an entry whose path does not exist → doctor output fails that binding and contains `herdr worktree remove --workspace <id>` and `zerdr workspace bind`.

**Complete when:** the case passes and no other doctor checks change.

**Validation:**
- Run: `cargo test --test setup_and_doctor`
- Expected: pass.

### Task 4: Rename `setup auto on|off` to `enable|disable`

**Covers:** R6, decision to drop aliases (Out of Scope)

**Objective:** The CLI and every hint string use `enable`/`disable`; `on`/`off` fail parsing.

**Files:**
- Modify: `src/cli.rs`, `src/lib.rs`, `src/setup.rs`, `src/doctor.rs`
- Test: `tests/cli_contract.rs`, `tests/setup_and_doctor.rs`

**Dependencies:** none

**Implementation notes:**
- `AutoState { Enable, Disable }` with updated doc comments; update `src/setup.rs:247` and `src/doctor.rs:328,334,356`; grep for remaining `auto on`/`auto off` in `src/` and tests.
- Flag-file behavior and Zed-settings ownership logic are untouched.

**Test cases:**
- `zerdr setup auto enable` / `disable` parse and flip the flag file (adapt existing on/off tests).
- `zerdr setup auto on` → clap error listing `enable, disable` as possible values.
- Doctor's disabled hint contains `zerdr setup auto enable`.

**Complete when:** `rg "auto (on|off)"` over `src/` and `tests/` returns nothing but historical docs/plans.

**Validation:**
- Run: `cargo test --test cli_contract && cargo test --test setup_and_doctor`
- Expected: pass.

### Task 5: Disabled-auto notice

**Covers:** R7, D4

**Objective:** `zerdr connect --auto` with the mode disabled prints the one-line notice and exits 0 without touching Herdr.

**Files:**
- Modify: `src/thread.rs` (`run_auto`)
- Test: `tests/cli_contract.rs`

**Dependencies:** Task 4 (the notice names `zerdr setup auto enable`).

**Implementation notes:**
- Replace the silent `return Ok(())` at `src/thread.rs:39-41` with a stdout println of the R7 string; keep exit 0 so `terminal_init_command` never breaks a thread.
- Rework `connect_auto_is_silent_when_disabled_without_touching_herdr`: it now asserts the notice on stdout, empty stderr, and (unchanged) that the fake `herdr` log stays empty; rename the test to match.

**Test cases:**
- Flag file absent, `connect --auto` → exit 0; stdout contains "auto mode is disabled" and `zerdr setup auto enable`; herdr log empty.
- Flag file present → no notice; existing enabled-path stdout unchanged.

**Complete when:** both cases pass.

**Validation:**
- Run: `cargo test --test cli_contract && cargo test --test thread_flow`
- Expected: pass.

### Task 6: README update

**Covers:** R8

**Objective:** README reflects the new behavior for people installing and using zerdr.

**Files:**
- Modify: `README.md`

**Dependencies:** Tasks 1-5 (documents their final behavior).

**Implementation notes:**
- Extend the `--create` sentence in "Terminal threads" (and the auto-mode paragraph) to say a checkout that is a linked Git worktree — whatever tool created it — is registered as a Herdr worktree-backed workspace, so `herdr worktree list`/`remove` manage it; keep the README user-focused per `AGENTS.md`.
- Replace `zerdr setup auto on`/`off` with `enable`/`disable`; mention the notice printed while the mode is disabled but the init command is installed.

**Test cases:** N/A (prose); proofread against implemented behavior.

**Complete when:** README contains no `setup auto on`/`off` and describes worktree registration accurately.

**Validation:**
- Run: `rg -n "auto (on|off)" README.md`
- Expected: no matches.

## Requirement Coverage

| Requirement / Decision | Task | Verification |
|---|---|---|
| R1 | Task 1 | worktree-create and auto-create cases assert `worktree open` argv, binding, attach |
| R2 | Task 1 | plain-checkout case asserts unchanged `workspace create` argv |
| R3 | Task 1 | failure case asserts nonzero exit, surfaced stderr, no fallback, no binding |
| R4 | Task 2 | bare-connect worktree case asserts updated message; plain case unchanged |
| R5 | Task 3 | doctor case asserts `herdr worktree remove` + rebind guidance |
| R6 | Task 4 | enable/disable parse tests, `on` rejection, hint-string assertions |
| R7 | Task 5 | disabled-auto case asserts notice, exit 0, empty herdr log |
| R8 | Task 6 | README grep + proofread |
| D1 | Task 1 | detection helper used only on the create branch; worktree fixtures built with plain `git worktree add` |
| D2 | Task 1 | `worktree open` argv contains no `--label`; status line asserts response label |
| D3 | Task 3 | doctor test mutates no state; message-only |
| D4 | Task 5 | notice printed on every disabled `--auto` run (test reruns the command) |

## Final Validation

- [ ] `cargo test --test thread_flow && cargo test --test cli_contract && cargo test --test setup_and_doctor` — Expected: pass
- [ ] `cargo fmt --all -- --check` — Expected: no diff
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` — Expected: no warnings
- [ ] `cargo test --all-targets --all-features` — Expected: pass
- [ ] Manual (developer, real environment — not automated): reinstall the binary; in a `wt`-created worktree with no Herdr workspace run `zerdr connect --create` and confirm the sidebar shows a worktree-backed workspace grouped with the repo and `herdr worktree list` includes it; open a new Zed terminal thread with auto disabled and confirm the notice
- [ ] Requirement Coverage has no unmapped rows
- [ ] Plan matches the actual changes
- [ ] After all of the above succeed, move this file unchanged to `docs/plans/archived/`

## Risks and Open Questions

- `herdr worktree open` response shape is documented only indirectly (Assumptions); the manual validation step confirms it, and only `worktree_open_for`'s pointers change if it differs.
- Behavior of `worktree open` when the parent repo has no Herdr workspace is Herdr's to define; per R3 zerdr surfaces whatever error Herdr returns rather than working around it.
- Real `git worktree add` in test fixtures adds a few hundred ms per test; keep it out of the shared fake and inside only the tests that need it (wrapper tests are time-budgeted per `AGENTS.md`).
