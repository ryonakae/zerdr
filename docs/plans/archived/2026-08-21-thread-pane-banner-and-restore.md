# Thread Pane Banner and Restore Implementation Plan

> **For implementers:** Execute tasks in order unless dependencies allow otherwise. Mark a task complete only after its validation succeeds. Reflect minor implementation differences in the relevant task. Ask the user before changing requirements, Out of Scope, or public contracts.

## Problem Statement

Two gaps remain after the auto-mode refinements. Inside an attached thread the pane's
shell looks identical to a plain Zed shell — the pre-attach status line is not part of
the pane content, so once the Herdr client renders there is no in-view marker. And after
a Zed restart, restored threads reattach to agent panes but not to plain-shell panes
zerdr created earlier: those have no agent for the resolution to find, so every restart
piles up fresh Herdr tabs.

## Goal

- Every pane zerdr creates carries one visible comment line in its shell saying it is a
  Herdr pane opened from a Zed terminal thread.
- zerdr remembers which panes its threads were attached to and, when resolving a bare
  thread, reattaches to a remembered live shell pane before creating a new tab — so a
  Zed restart restores the previous panes instead of multiplying tabs.

## Out of Scope

- Injecting anything into panes zerdr did not create (an existing agent pane's input
  belongs to the agent; the pre-attach status line keeps covering that case).
- Restoring an exact thread-to-pane mapping (restored threads are indistinguishable;
  restoring the set of panes is the achievable contract).
- Herdr-side labels (`tab rename` / `pane rename`) — declined in dig in favor of the
  in-pane comment line.
- Changing the priority of free agents (they stay first, per dig Q3-A).

## Requirements and Decisions

### Requirements

- **R1:** Immediately after zerdr creates a pane (the fresh-tab pane and the root pane
  of a created workspace), it sends one shell comment line into that pane via Herdr —
  `pane send-text` with a `# zerdr: ...` string followed by `pane send-keys ... enter`.
  The text names the pane id, the workspace label (or id), and that it was opened from
  a Zed terminal thread. It applies to manual and auto paths alike.
- **R2:** The banner is never sent to a pane zerdr did not create in this invocation
  (existing agent panes, remembered panes being reattached).
- **R3:** Banner delivery is best-effort: a failure prints a `zerdr:` warning to stderr
  and the attach proceeds.
- **R4:** Every successful attach records (workspace id, pane id, timestamp) in a
  persistent per-session/socket store under zerdr's state directory.
- **R5:** Bare-thread resolution order becomes: ① a free agent in the matching
  workspace (unchanged) → ② a remembered pane in that workspace that is still alive,
  not currently leased, and not hosting a live agent (attached via its terminal, most
  recently attached first) → ③ a fresh tab. Manual `zerdr thread` and `--auto` share
  this order.
- **R6:** A remembered pane that no longer exists is dropped from the store when
  resolution encounters it; records never make resolution fail (a broken store entry
  degrades to the next candidate).
- **R7:** The status line (previous plan's R4) also covers the remembered-pane outcome:
  it identifies the pane and workspace and reads as a reattach to an existing Herdr
  pane, not a new tab.
- **R8:** README documents the banner and the restore behavior.

### Implementation Decisions

- **D1:** The banner is a shell comment (`# ...`), not an executed command: it renders
  as one line at the prompt with no side effects (dig Q2-B).
- **D2:** Memory reads and writes happen under the existing per-session resolve lock
  (`ThreadLeaseSet::resolve_lock_path`) so two threads racing on the same workspace
  cannot claim the same remembered pane; the explicit-`TARGET` path records its pane
  under the same lock. Store writes use the existing atomic-write helper.
- **D3:** The store lives beside the thread leases: one JSON document per
  session+socket scope (same scope hashing as `ThreadLeaseSet`), with a
  `schema_version` and a list of `{workspace_id, pane_id, last_attached_unix_ms}`
  records, deduplicated by pane id (an existing record's timestamp is refreshed on
  reattach).
- **D4:** Liveness of a remembered pane is checked with `pane get` (which also yields
  the `terminal_id` needed for the attach); a failing `pane get` means dead → prune.
  Panes listed by `agent list` are skipped in pass ② because pass ① owns them.
- **D5:** The fake herdr's existing catch-all already logs `pane send-text` /
  `pane send-keys` and exits 0, so no new success-path handlers are added. The only
  new hooks are an env switch to make `pane get` fail for listed pane ids
  (`ZERDR_TEST_PANE_GET_MISSING_IDS`) and a banner-failure injection knob (e.g.
  `ZERDR_TEST_SEND_TEXT_EXIT`). Both are conditional branches only — the shared fake
  stays free of unconditional per-invocation work, and the wrapper-test timing budget
  is re-verified (AGENTS.md constraint).

### Contracts

- Banner string: starts with `# zerdr:` and contains the pane id, the workspace label
  or id, and the words "Zed terminal thread"; exact wording free.
- Herdr calls for the banner: `pane send-text <pane> <text>` then
  `pane send-keys <pane> enter`, in that order, before the attach child spawns.
- Memory store schema (per scope file): `{ schema_version: 1, panes: [{workspace_id,
  pane_id, last_attached_unix_ms}] }`. Unknown/foreign content in the file is treated
  as absent (start fresh) rather than an error.
- `uninstall --purge` keeps removing the whole state dir, memory included (no change).

## Current Context

### Confirmed

- Herdr CLI exposes `pane send-text <pane_id> <text>`, `pane send-keys <pane_id> <key>...`,
  and `pane get <pane_id>` (verified via `herdr pane` help). zerdr already wraps
  `pane get` as `pane_terminal_for` (src/herdr.rs:183).
- `resolve_or_create` (src/thread.rs) holds `OperationGuard` on
  `resolve_lock_path(session, socket)` for the whole bare resolution; pass ① (free
  agents) and pass ③ (tab create) are where ② slots in. The explicit-`TARGET` arm in
  `run_with_mode` bypasses it.
- The fake herdr's `pane get` currently always succeeds, synthesizing
  `terminal_id: "term-<pane_id>"`. Unmatched subcommands are already logged by the
  unconditional log line at the top of `FAKE_HERDR_BODY` and answered by the catch-all
  case with `{"ok":true,...}` exit 0 — so success-path banner assertions need no new
  handlers; the only genuinely new fake hooks are `ZERDR_TEST_PANE_GET_MISSING_IDS`
  and a banner-failure injection knob.
- `ThreadLeaseSet` provides the scope-hashing (session+socket) and `leased_panes`;
  `atomic_write_json` and `now_millis` exist in src/state.rs.
- `Attachment` enum and `print_status` (src/thread.rs) currently know
  `Agent` / `NewTab` / `NewWorkspace { label }`.
- AGENTS.md: the shared fake herdr must stay lean; wrapper tests budget ~2s under
  parallel load. `Date`-free: tests must not depend on wall-clock ordering finer than
  the store's own timestamps.

### Assumptions

- The banner comment is sent before the thread attaches; the shell renders it on the
  prompt line, and it scrolls like normal content afterwards. Whether fish keeps
  comment-only lines out of history is fish's behavior, not zerdr's contract.
- Store file name and exact `Attachment` variant naming are implementation details.

## File Structure

- Modify: `src/herdr.rs` — `pane_send_text_for`, `pane_send_keys_for` wrappers.
- Modify: `src/state.rs` — thread-pane memory store (scope-hashed persistence, load /
  record / prune operations).
- Modify: `src/thread.rs` — banner after pane creation; memory pass ② in
  `resolve_or_create`; recording on every successful attach; status-line variant for
  the reattach outcome.
- Test: `tests/support/mod.rs` — the `ZERDR_TEST_PANE_GET_MISSING_IDS` and
  `ZERDR_TEST_SEND_TEXT_EXIT` hooks in `FAKE_HERDR_BODY` (no success-path handlers).
- Test: `tests/thread_flow.rs` — banner assertions; restore scenarios.
- Test: `tests/state_and_bindings.rs` — memory-store persistence behavior (dedup,
  refresh, foreign-content tolerance) if a direct seam is cleaner than end-to-end.
- Modify: `README.md` — terminal-threads section: banner and restore.

## Testing Decisions

- **Test seam:** compiled binary + fake herdr (`Fixture` in tests/thread_flow.rs) for
  behavior; direct store API in tests/state_and_bindings.rs for persistence edge cases.
- **Behavior:** fake-herdr call log (`pane send-text` / `pane send-keys` / `pane get` /
  `terminal attach` / `tab create` ordering and absence), stdout status lines, store
  file contents.
- **Prior art:** `an_empty_workspace_gets_a_new_tab_with_a_plain_shell` (tab-create
  fixtures), `a_second_thread_picks_a_different_agent_pane` (sequential runs),
  `the_resolve_lock_is_scoped_to_one_session_and_socket`.
- **Avoid:** asserting exact banner wording beyond the contract substrings; depending
  on timestamp values.

## Progress

- [x] Task 1: In-pane banner for created panes
- [x] Task 2: Pane memory and restore
- [x] Task 3: Documentation

## Tasks

### Task 1: In-pane banner for created panes

**Covers:** R1, R2, R3, D1, D5 (send-text/send-keys part)

**Objective:** a pane zerdr creates shows one `# zerdr: ...` comment line in its shell;
panes zerdr merely attaches to receive nothing.

**Files:**
- Modify: `src/herdr.rs`, `src/thread.rs`
- Test: `tests/support/mod.rs` (only if the fake needs explicit handlers), `tests/thread_flow.rs`

**Dependencies:** none

**Implementation notes:**
- Send the banner right after `tab_create_for` / `workspace_create_for` return the
  pane, inside `resolve_or_create` (the workspace label for the wording is in scope
  there). Failures warn on stderr and continue (R3).
- The fake's catch-all already logs and succeeds for `pane send-text`/`send-keys`, so
  add no dead handlers; only add the failure-injection hook needed by the R3 test.
  Keep additions conditional and rerun the wrapper tests to confirm the timing budget.

**Test cases:**
- fresh tab (`an_empty_workspace_gets_a_new_tab_with_a_plain_shell`) → log contains
  `pane send-text w1:p9 # zerdr:` (with "Zed terminal thread" and the workspace in the
  text) followed by `pane send-keys w1:p9 enter`, before `terminal attach term-w1:p9`.
- created workspace (manual `--create` and auto-create tests) → same for `w7:p1`.
- attach to existing agent → log contains no `send-text` / `send-keys`.
- banner failure (make `send-text` fail via a new env hook, e.g.
  `ZERDR_TEST_SEND_TEXT_EXIT=1`) → attach still succeeds, one stderr warning.
- created pane + `--kind pi` (or `ZERDR_THREAD_KIND`) → banner send-text/send-keys
  appear before `agent start` in the log, and the attach still succeeds.

**Complete when:**
- Banner asserted for both creation paths, absence asserted for plain attach.
- `cargo test --test thread_flow --test herdr_wrapper --all-features` passes
  (herdr_wrapper included to catch fake-timing regressions).

**Validation:**
- Run: `cargo test --test thread_flow --test herdr_wrapper --all-features`
- Expected: all tests pass.

**Result:** Done. `send_banner` in src/thread.rs (called after `workspace_create_for` and
after `tab_create_for`, before `start_and_lease` so the comment lands before any
`agent start`); `pane_send_text_for`/`pane_send_keys_for` wrappers in src/herdr.rs; the
fake herdr gained only the conditional `ZERDR_TEST_SEND_TEXT_EXIT` branch. thread_flow
(28) and herdr_wrapper (20, timing budget intact) green; fmt/clippy clean.

### Task 2: Pane memory and restore

**Covers:** R4, R5, R6, R7, D2, D3, D4, D5 (pane-get hook part)

**Objective:** a second bare thread (or a restored one after Zed restarts) reattaches
to the remembered shell pane instead of creating another tab.

**Files:**
- Modify: `src/state.rs`, `src/thread.rs`
- Test: `tests/support/mod.rs` (`ZERDR_TEST_PANE_GET_MISSING_IDS`), `tests/thread_flow.rs`,
  `tests/state_and_bindings.rs`

**Dependencies:** Task 1 (shared thread.rs edits; banner absence on reattach)

**Implementation notes:**
- Store operations: load (tolerating foreign/invalid content as empty), record
  (dedup by pane id, refresh timestamp), prune (remove listed pane ids). All called
  under the resolve lock; the explicit-`TARGET` path takes the lock just to record.
- Pass ② iterates remembered panes for the matched workspace, most recent first,
  skipping leased panes and panes present in the `agent list` snapshot; the first one
  whose `pane get` succeeds is attached via its terminal id; failures prune and move on.
- Reattach must NOT send the banner (R2) and must record (refresh) the pane (R4).
- `print_status`: reattach outcome reads as attaching to an existing Herdr pane.

**Test cases:**
- run 1 creates a tab (w1:p9) and exits; run 2 (same fixture state) → log shows
  `pane get w1:p9` and `terminal attach term-w1:p9`, exactly one `tab create` across
  both runs, no `send-text` in run 2, and run 2's status line reads as a reattach.
- remembered pane dead (`ZERDR_TEST_PANE_GET_MISSING_IDS=w1:p9`) → run 2 creates a new
  tab and the store no longer lists w1:p9.
- free agent present + remembered shell pane → the agent wins (order R5-①).
- remembered pane in another workspace is not considered (record for w2 while
  resolving w1 → new tab).
- recency among multiple remembered panes: run 1 attaches (and holds) the created tab
  w1:p9, run 2 starts while run 1 is attached so it creates w1:p10, both exit; run 3
  reattaches `term-w1:p10` (the most recently attached) and creates no tab.
- explicit `zerdr thread TARGET` writes a memory record: after a target attach exits,
  a store file under the state dir lists that pane id (covers the lock-taking record
  path outside `resolve_or_create`).
- store-level (tests/state_and_bindings.rs): record dedups by pane id and refreshes
  recency; foreign JSON in the store file loads as empty without erroring.

**Complete when:**
- The restart scenario (run 1 → run 2 reattach) and all edge cases above pass.
- `cargo test --all-targets --all-features` passes.

**Validation:**
- Run: `cargo test --test thread_flow --test state_and_bindings --all-features && cargo test --all-targets --all-features`
- Expected: all tests pass.

**Result:** Done. `ThreadPaneMemory` in src/state.rs (scope path = hash(socket + NUL +
session) like the lease scopes; tolerant `load`, dedup-refresh `record`, `prune`);
`Paths.thread_memory_dir` (`<state>/thread-panes`). thread.rs records on all five attach
sites (`remember_pane`, advisory with a stderr warning on failure), and pass ② sits
between the free-agent pass and tab creation inside the resolve lock; the explicit-TARGET
arm takes the resolve lock briefly just to record (no deadlock: pane-lease acquisition is
non-blocking try-lock, so lock-order inversion cannot block). New `Attachment::Remembered`
prints "reattached to Herdr pane ...". Fake herdr gained the conditional
`ZERDR_TEST_PANE_GET_MISSING_IDS` branch. All six planned scenarios covered
(tests/thread_flow.rs) plus two store tests (tests/state_and_bindings.rs). Full suite
green (200 tests at --test-threads=4), fmt/clippy clean.

### Task 3: Documentation

**Covers:** R8

**Objective:** README reflects the banner and restore behavior.

**Files:**
- Modify: `README.md`

**Dependencies:** Tasks 1–2

**Implementation notes:**
- Terminal-threads section: the comment-line banner in created panes; reattach-on-
  restart now covers plain-shell panes too (update the resume sentence); note that a
  thread you close leaves its pane available for the next thread to pick up.

**Test cases:**
- N/A (prose); cross-check against implemented behavior.

**Complete when:**
- README matches behavior; `rg -n "banner|comment|restore|reattach" README.md` shows
  the additions.

**Validation:**
- Run: `rg -n "# zerdr|reattach" README.md`
- Expected: the banner and restore descriptions are present.

**Result:** Done. The status-line paragraph now mentions the banner and the reattached
outcome; the resume paragraph describes the remembered-pane order (agent → remembered
pane → fresh tab) and the closed-thread pickup behavior. Validation grep confirms both.

## Requirement Coverage

| Requirement / Decision | Task | Verification |
|---|---|---|
| R1 | Task 1 | send-text/send-keys log assertions for both creation paths |
| R2 | Tasks 1, 2 | absence assertions on plain attach and on reattach |
| R3 | Task 1 | banner-failure injection test (attach succeeds, one warning) |
| R4 | Task 2 | store contents after attach; explicit-TARGET record test; refresh-on-reattach store test |
| R5 | Task 2 | reattach scenario; recency-order test with two remembered panes; agent-priority test; cross-workspace test |
| R6 | Task 2 | dead-pane prune test |
| R7 | Task 2 | reattach status-line assertion |
| R8 | Task 3 | README grep + read-through |
| D1–D5 | Tasks 1–2 | design constraints reflected in task notes (code review) |

## Final Validation

- [x] `cargo fmt --all -- --check` — Expected: no diff — clean
- [x] `cargo clippy --all-targets --all-features -- -D warnings` — Expected: no warnings — clean
- [x] `cargo test --all-targets --all-features` — Expected: all tests pass — 202 tests green
      (run at `--test-threads=4`; GitHub CI green on HEAD 2e4b45e on macOS and Ubuntu)
- [x] Manual check — outcome recorded: the restore half passed (the user confirmed the
      restored thread reattaches to the same pane). The banner half worked as specified
      but was rejected on sight: the comment arrives before the shell starts (double
      echo) and fish highlights it red like an error. The banner was therefore removed
      and replaced by the `[herdr]` sidebar title marker in the follow-up plan
      `2026-08-21-thread-title-marker.md`; the pane memory/restore feature from this
      plan remains in place.
- [x] Requirement Coverage に未対応項目がない
- [x] 計画と実際の変更内容が整合している（バナー部分はその後 title-marker プランで置換）
- [x] 上記のすべてが成功した後、計画を同名のまま `docs/plans/archived/` へ移した

**Independent review:** two rounds by a separate reviewer context. Round 1: ISSUES FOUND —
one medium (control characters in a workspace label or checkout name could turn the
banner into executed shell input, violating D1) and one low (the banner-before-agent-start
ordering was untested on the workspace-creation path). Both fixed in 2e4b45e
(`single_line` flattening of every interpolated banner part + two new tests). Round 2:
APPROVED, no remaining issues; the reviewer noted the fix also covers escape-sequence
injection and that the pane-addressing argument correctly stays unflattened.
Implementation commits: 9068e9d, 147a5bb, e002a07, 2e4b45e (base a850a83), all pushed,
CI green on HEAD.

## Risks and Open Questions

- Sending the banner types into the pane's shell; if a future creation path ever
  yields a pane that is not at a fresh prompt, the comment would land in that
  program's input. Today both creation paths produce fresh shell panes, and R2 keeps
  the banner off every other pane.
- The memory store extends the unlocked-install.json concern's family, but here all
  mutation happens under the resolve lock (D2), so no new cross-command race is added.
- Remembered panes the user closed in Herdr are pruned only when resolution touches
  them; the store may briefly list dead panes between runs (harmless by R6).
- No open questions.
