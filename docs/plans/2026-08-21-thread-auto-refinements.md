# Thread Auto Mode Refinements Implementation Plan

> **For implementers:** Execute tasks in order unless dependencies allow otherwise. Mark a task complete only after its validation succeeds. Reflect minor implementation differences in the relevant task. Ask the user before changing requirements, Out of Scope, or public contracts.

## Problem Statement

Real-world use of thread auto mode surfaced three gaps: opening a thread in a project
without a Herdr workspace dead-ends in a plain shell instead of just working; the
sidebar title format `"<kind> · <title>"` diverges from what Zed shows natively for a
terminal-thread agent; and a freshly opened thread gives no clue whether it is a Herdr
pane or a plain local shell, nor whether auto mode is on.

## Goal

- `zerdr thread --auto` creates (and binds) the Herdr workspace when none matches.
- The injected OSC title matches Zed's native terminal-thread rendering: the pane's
  title verbatim, no agent-kind prefix.
- Every thread start prints one status line saying what happened (attached / new tab /
  new workspace / plain shell) with pane and workspace info, and — on the auto path —
  the mode state.

## Out of Scope

- Mouse-drag selection: no zerdr change; Zed's native Shift+drag escape hatch is the
  answer (documented in README as a tip only).
- Changing manual `zerdr thread` workspace-creation semantics (still explicit `--create`).
- Detach-time status lines (revisit if the pre-attach line proves invisible in practice).
- Deviating further from native titles (custom glyphs, icons).

## Requirements and Decisions

### Requirements

- **R1:** `zerdr thread --auto` with no matching workspace creates one exactly like
  manual `--create` (git root as cwd, directory name as label, binding recorded, fresh
  pane as a plain shell unless `ZERDR_THREAD_KIND` is set). Manual `zerdr thread`
  keeps failing with the existing guidance unless `--create` is passed.
- **R2:** Auto-path failures that remain (not a Git checkout, Herdr unavailable, herdr
  call errors) keep the best-effort contract: one `zerdr: ...` stderr line, exit 0.
- **R3:** The OSC title is the agent's `terminal_title_stripped` verbatim — no
  `"<kind> · "` prefix in any case. When the title is empty or absent, fall back to
  the workspace label, then the workspace id (existing fallback chain, unchanged).
  Rationale: Zed shows the raw OSC title verbatim and auto-promotes a single leading
  decorative glyph to the row icon; any textual prefix breaks that.
- **R4:** Every successful thread start prints exactly one human-readable status line
  before attaching, stating the outcome and identifying the pane and workspace:
  - attached to an existing agent → outcome "attached", with the agent kind, pane id,
    and workspace label (id when the label is unknown);
  - opened a fresh tab holding a plain shell → outcome "new tab", with pane id and
    workspace label/id;
  - created a new workspace → outcome "created workspace", with label and pane id;
  - an explicit `TARGET` attach prints the attached line too.
- **R5:** On the `--auto` path the status line also states the mode: disabled →
  `zerdr: thread auto mode is disabled; this is a plain local shell` (replacing the
  silent no-op; still exit 0, still no Herdr calls); enabled → the R4 outcome line
  carries an "auto mode is enabled" clause. Manual `zerdr thread` prints the R4 line
  without any mode clause.
- **R6:** README documents the new auto-create behavior, the status lines, and a tip
  that Shift+drag selects text while the Herdr client has mouse reporting active
  (Zed v1.16.1+).

### Implementation Decisions

- **D1:** Reuse the existing `create=true` path: `run_auto` simply invokes the shared
  flow with `create` forced on. No second creation code path.
- **D2:** Status lines go to stdout (they are output, not errors); the existing
  best-effort failure lines stay on stderr. Exact wording is the implementer's choice
  as long as R4/R5's required facts are present and it is one line.
- **D3:** The resolution outcome (existing agent / new tab / new workspace / explicit
  target) must reach the printing site in `run` — e.g. a small enum produced by both
  the `resolve_or_create` call and the explicit-`TARGET` match arm (src/thread.rs:57,
  which bypasses `resolve_or_create` entirely) — so the line is printed once, after
  the workspace label is known and before the attach child is spawned. Exact shape is
  free.
- **D4:** Keep the empty-title fallback (workspace label) even though native Zed would
  show an empty string: for a plain-shell tab it is the only hint of where the thread
  is attached. Confirmed with the user.

### Contracts

- `emit_title` labeling: `title.or(label).unwrap_or(workspace_id)`, emitted verbatim.
- Auto-path stdout on success: exactly one status line, then OSC/bell output as today.
- Disabled `--auto`: one stdout line, exit 0, no Herdr invocations, no state changes.
- Everything else about leases, focus, polling, and bells is unchanged.

## Current Context

### Confirmed

- `run_auto` (src/thread.rs:27) currently passes `create=false` and is silent when the
  mode is off; `resolve_or_create` (src/thread.rs:115) already implements creation,
  binding, and the plain-shell default behind `create`.
- `emit_title` (src/thread.rs:391) prepends `"{kind} · "` whenever `kind` is non-empty.
- Zed native behavior (verified in zed-industries/zed source): the agent panel shows
  `TerminalThreadMetadata.title` — the raw OSC 0/2 title verbatim, no prefixes; a
  single leading non-alphanumeric glyph + whitespace is promoted to the row icon
  (`crates/sidebar/src/sidebar.rs::split_leading_icon_char`); empty titles fall back
  to a derived process title or the empty string.
- Zed selects text under application mouse mode with Shift+drag (hardcoded escape
  hatch, `crates/terminal/src/terminal.rs::mouse_mode`; regression fixed in v1.16.1).
- Existing tests asserting the old contracts:
  `thread_auto_is_a_silent_no_op_while_the_mode_is_disabled` (tests/cli_contract.rs),
  `auto_without_a_matching_workspace_leaves_a_plain_shell_with_one_note`,
  `bare_thread_attaches_a_free_agent_in_the_matching_workspace` (asserts `pi · review`),
  `titles_are_emitted_once_per_change_and_a_bell_marks_settling`,
  `an_empty_title_falls_back_to_the_workspace_label`,
  `auto_attaches_a_free_agent_while_the_mode_is_enabled` (asserts `pi · review` and an
  empty stderr) in tests/thread_flow.rs.
- The fake herdr already supports `workspace create` and `tab create`
  (tests/support/mod.rs), used by `create_makes_the_workspace_binds_it_and_starts_an_agent`.

### Assumptions

- The pre-attach status line may be hidden while the Herdr client renders; it remains
  in scrollback and after detach. Whether that is visible enough is settled by the
  user's manual check, and a follow-up (e.g. detach-time line) is out of scope for now.

## File Structure

- Modify: `src/thread.rs` — `run_auto` create + disabled line; outcome propagation from
  `resolve_or_create`; status-line printing in `run`; `emit_title` verbatim titles.
- Test: `tests/thread_flow.rs` — auto-create test replaces the no-match note test;
  title assertions drop the kind prefix; status-line assertions.
- Test: `tests/cli_contract.rs` — disabled `--auto` now prints one line (still exit 0,
  empty stderr, no herdr calls).
- Modify: `README.md` — terminal-threads section: auto-create, status lines, Shift+drag tip.

## Testing Decisions

- **Test seam:** the compiled binary against fake herdr fixtures (`TestEnv`,
  `Fixture` in tests/thread_flow.rs), as today.
- **Behavior:** stdout status lines and OSC payloads, stderr for best-effort failures,
  fake-herdr call log for created workspaces/tabs and absence of calls when disabled.
- **Prior art:** `create_makes_the_workspace_binds_it_and_starts_an_agent` for the
  creation assertions; `an_empty_workspace_gets_a_new_tab_with_a_plain_shell` for
  plain-shell assertions.
- **Avoid:** asserting exact human wording beyond the required facts (match on stable
  substrings like pane ids, workspace labels, "auto mode", not full sentences).

## Progress

- [x] Task 1: Auto-create the workspace on `--auto`
- [ ] Task 2: Native verbatim titles
- [ ] Task 3: Status lines
- [ ] Task 4: Documentation

## Tasks

### Task 1: Auto-create the workspace on `--auto`

**Covers:** R1, R2, D1

**Objective:** opening a thread with auto mode on in a Herdr-less project creates and
binds the workspace and lands in a plain shell inside it; manual behavior unchanged.

**Files:**
- Modify: `src/thread.rs`
- Test: `tests/thread_flow.rs`

**Dependencies:** none

**Implementation notes:**
- `run_auto` passes `create: true` into the shared flow. The manual no-match error and
  its `--create` hint stay untouched for `zerdr thread`.
- Auto in a non-Git directory still fails best-effort (existing `canonical_git_root`
  error → one stderr line, exit 0).

**Test cases:**
- auto enabled + empty workspace list → log contains `workspace create` with the
  fixture root and label, binding recorded for the new workspace id, exit 0 after
  detach; replaces `auto_without_a_matching_workspace_leaves_a_plain_shell_with_one_note`.
- auto enabled + non-Git cwd → exit 0, single stderr line, no `workspace create`.
- manual `zerdr thread` with no match → unchanged error (existing test keeps passing).

**Complete when:**
- The new auto-create test passes and the manual no-match test is untouched.
- Validation succeeds.

**Validation:**
- Run: `cargo test --test thread_flow --all-features`
- Expected: all tests pass.

**Result:** Done. One-line change in `run_auto` (`create: true`); tests replaced/added as
planned (`auto_without_a_matching_workspace_creates_and_binds_one`,
`auto_outside_a_git_checkout_leaves_a_plain_shell_with_one_note`). thread_flow green (27).

### Task 2: Native verbatim titles

**Covers:** R3, D4

**Objective:** the sidebar shows exactly what the agent's terminal title says, matching
Zed-native rendering.

**Files:**
- Modify: `src/thread.rs` (`emit_title`; the now-unused kind handling)
- Test: `tests/thread_flow.rs`

**Dependencies:** none

**Implementation notes:**
- Label = `agent.title` verbatim, falling back to the workspace label, then the
  workspace id. Dedup-on-change and bell logic unchanged.

**Test cases:**
- attach to agent titled "review the diff" → OSC payload is exactly `review the diff`
  (no `pi · `); update `bare_thread_attaches_a_free_agent_in_the_matching_workspace`,
  `auto_attaches_a_free_agent_while_the_mode_is_enabled`,
  `titles_are_emitted_once_per_change_and_a_bell_marks_settling`.
- empty title → workspace label emitted (existing fallback test keeps passing,
  adjusted for the removed prefix if needed).

**Complete when:**
- No OSC payload anywhere contains the `" · "` kind separator.
- Validation succeeds.

**Validation:**
- Run: `cargo test --test thread_flow --all-features`
- Expected: all tests pass.

### Task 3: Status lines

**Covers:** R4, R5, D2, D3

**Objective:** every thread start says what it did; the auto path also says whether the
mode is on or off.

**Files:**
- Modify: `src/thread.rs`
- Test: `tests/thread_flow.rs`, `tests/cli_contract.rs`

**Dependencies:** Task 1 (auto-create is one of the reported outcomes)

**Implementation notes:**
- Propagate the resolution outcome out of `resolve_or_create` (D3) and print the line
  in `run` after the workspace label lookup, before spawning the attach child.
- `run` needs to know it is on the auto path to add the mode clause — e.g. an `auto`
  flag parameter or a wrapper that decorates the line; implementer's choice.
- The disabled branch in `run_auto` prints its line on stdout and still returns before
  any Herdr call.

**Test cases:**
- disabled `--auto` → stdout is exactly one line containing "auto mode" and "disabled",
  stderr empty, exit 0, empty herdr log (update
  `thread_auto_is_a_silent_no_op_while_the_mode_is_disabled`, renamed accordingly).
- enabled `--auto` attaching to a free agent → stdout contains one status line with
  "enabled", the pane id `w1:p1`, and the workspace label before the first OSC payload.
- manual `zerdr thread` attaching → status line with pane id and label, and no
  "auto mode" text.
- manual thread on an empty workspace (new tab) → line identifies the new pane id.
- auto-create (Task 1 fixture) → line mentions the created workspace label and pane id.
- explicit `zerdr thread TARGET` → status line with the pane id (extend
  `an_explicit_target_attaches_without_creating_anything`).

**Complete when:**
- All six outcomes above are asserted, including the explicit-`TARGET` path that does
  not go through `resolve_or_create`.
- Validation succeeds.

**Validation:**
- Run: `cargo test --test thread_flow --all-features && cargo test --test cli_contract --all-features`
- Expected: all tests pass.

### Task 4: Documentation

**Covers:** R6

**Objective:** README reflects auto-create, the status lines, and the selection tip.

**Files:**
- Modify: `README.md`

**Dependencies:** Tasks 1–3

**Implementation notes:**
- Terminal-threads section: `--auto` now creates missing workspaces (and what that
  means on restart); the status line tells plain shell from Herdr pane; add the
  Shift+drag tip with the Zed v1.16.1 note. `--create` stays documented for manual use.

**Test cases:**
- N/A (prose); cross-check statements against `zerdr thread --help` and test behavior.

**Complete when:**
- README matches the implemented behavior; validation succeeds.

**Validation:**
- Run: `rg -n "auto|Shift" README.md`
- Expected: the three additions present; no stale "never creates a workspace on auto" claims.

## Requirement Coverage

| Requirement / Decision | Task | Verification |
|---|---|---|
| R1 | Task 1 | auto-create test; manual no-match test unchanged |
| R2 | Task 1 | non-Git-cwd best-effort test; existing herdr-unavailable test |
| R3 | Task 2 | OSC payload assertions without the kind prefix; fallback test |
| R4 | Task 3 | status-line assertions for attach / new tab / create / manual / explicit TARGET |
| R5 | Task 3 | disabled-line test; enabled-clause assertions; manual line without mode text |
| R6 | Task 4 | README grep + read-through |
| D1–D4 | Tasks 1–3 | design constraints reflected in task notes (code review) |

## Final Validation

- [ ] `cargo fmt --all -- --check` — Expected: no diff
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` — Expected: no warnings
- [ ] `cargo test --all-targets --all-features` — Expected: all tests pass
- [ ] Manual check (user's machine, after `cargo install --path . --locked --force`):
      open a thread in a project without a Herdr workspace while enabled → workspace
      appears in Herdr and the thread lands in its shell with a status line; sidebar
      title shows the agent's own title without a `pi ·` prefix; disabled mode prints
      the plain-shell line; Shift+drag selects text inside the attached pane.
- [ ] Requirement Coverage に未対応項目がない
- [ ] 計画と実際の変更内容が整合している
- [ ] 上記のすべてが成功した後、計画を同名のまま `docs/plans/archived/` へ移した

## Risks and Open Questions

- The pre-attach status line may be covered by the Herdr client's rendering; if the
  manual check finds it invisible, a detach-time line or Herdr-side notification is a
  follow-up decision (out of scope here).
- Auto-create means Zed-restored threads can create workspaces for any Git project a
  thread was open in; accepted in dig (mitigation: `zerdr thread --disable`).
- No open questions.
