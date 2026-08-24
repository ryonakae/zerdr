# Thread Detach/Attach Implementation Plan

> **For implementers:** Execute tasks in order unless dependencies allow otherwise. Mark a task complete only after its validation succeeds. Reflect minor implementation differences in the relevant task. Ask the user before changing requirements, Out of Scope, or public contracts.

## Problem Statement

A Zed terminal thread holds its Herdr pane through a direct attach client (`herdr agent|terminal attach`), which pins the pane's PTY to the thread terminal's size (typically 79×77, or 80×24 while the thread view is hidden). Any other Herdr client viewing that pane — notably a phone-sized SSH client (~42×19) — sees a pane laid out for the pinned size: clipped width, wrong height. Verified against herdr 0.8.2: a direct attach client resizes the shared pane to its own terminal size and holds it there until it disconnects; full (SemanticFrame) clients reflow per client and coexist fine.

The user needs a way to suspend every thread attach before working from the phone, and to restore them afterwards, without closing or re-creating the threads.

## Goal

`zerdr detach` terminates every live thread's attach client (all sessions) while each `zerdr connect` process keeps running, keeps its lease, and keeps mirroring the agent title. `zerdr attach` reconnects every surviving thread to its original pane. Both commands are synchronous and idempotent, and work from an SSH session on the same machine.

## Out of Scope

- Per-session or per-thread filtering (`--session` stays rejected for these commands; add later if needed).
- Any Herdr-side change (no-resize attach, letterboxing) — upstream feature territory.
- Auto-detach heuristics (e.g. reacting to Zed's 80×24 hidden-thread resize).
- `zerdr start` / full-client behavior — unaffected by design.
- Windows (direct terminal attach is Unix-only in Herdr; zerdr already targets Unix).

## Requirements and Decisions

### Requirements

- **R1:** `zerdr detach` enables a global detach mode and terminates the attach child of every live thread connect across all sessions. Connect processes, their leases, and their monitors stay alive.
- **R2:** `zerdr detach` waits until every live thread confirms it is detached (default timeout 5 s), then reports the count. On timeout it warns with the number of unconfirmed threads and exits non-zero.
- **R3:** While detach mode is on, no zerdr connect pins any Herdr pane. New connects (manual and auto) resolve their target, acquire the lease, print a notice, and start in detached wait without spawning an attach client.
- **R4:** `zerdr attach` disables detach mode; every waiting connect reattaches to its original pane by resolving the pane's current `terminal_id` and spawning `terminal attach` (works whether or not an agent is running — same pane-based semantics as today's "agent exited, still attached" behavior). The command waits and reports symmetrically to R2.
- **R5:** If the pane no longer resolves at reattach time, that connect prints a notice and exits gracefully (exit 0); other threads are unaffected.
- **R6:** Detach terminates the attach child with SIGTERM (not SIGKILL) so the Herdr client restores the thread terminal's modes (verified: alt-screen, mouse, kitty keyboard, bracketed paste are all restored on SIGTERM). After the child exits, connect prints a one-line notice telling the user to run `zerdr attach`.
- **R7:** While detached, the threads sidebar title keeps following the agent with a visible detached marker (`[herdr⏸]` replaces `[herdr]` in both title forms), and no bell is emitted (no Zed notifications while detached).
- **R8:** Both commands are idempotent: `zerdr detach` when already detached re-confirms and reports; `zerdr attach` when detach mode is not active prints a no-op notice; both exit 0.
- **R9:** `--session` with `detach`/`attach` is rejected with the existing "--session cannot be used with this command" error.
- **R10:** `zerdr detach` and `zerdr attach` run under SSH markers (`SSH_CONNECTION` etc.): they join `setup doctor` as exemptions from the remote-environment rejection. All other commands keep rejecting.
- **R11:** SIGINT/SIGTERM/SIGHUP during detached wait end the connect gracefully: lease and detach marker are released, and `zerdr attach` does not wait for that thread afterwards.
- **R12:** Existing connect behavior is unchanged when detach mode is never engaged: attach child exit still ends connect with the child's status; title and bell behavior are unchanged.

### Implementation Decisions

- **D1:** Detach mode is a flag file `Paths::thread_detach_flag_file` under the state dir (same pattern as `thread_auto_flag_file`). Existence = detached. One global flag (all sessions) — decided over per-session scoping for a one-shot phone workflow.
- **D2:** Per-thread detach confirmation is a sidecar marker next to the lease file (lease `<hash>.json` → marker `<hash>.detached`), created/removed only by the lease-holding connect. Existence-based, so readers never see torn writes; the lease JSON schema is untouched (no migration).
- **D3:** Reattach resolves the pane's terminal id at reattach time via the existing `Herdr::pane_terminal_for`, then `spawn_terminal_attach_for`. Any resolution failure takes the R5 graceful-exit path (no retry loop in v1).
- **D4:** Detach uses SIGTERM + wait on the attach child. `ManagedChild::terminate` (SIGKILL) stays reserved for drop/cleanup; add a graceful-terminate method beside it.
- **D5:** Connect polls the flag and the child at an interval ≤ 1 s (reuse `ZERDR_THREAD_POLL_MS` / `DEFAULT_POLL_MS`). Command wait timeout defaults to 5 s, overridable via `ZERDR_DETACH_WAIT_MS` (test seam, mirrors `ZERDR_READY_TIMEOUT_MS`).
- **D6:** `SignalForwarder` exists only while an attach child is alive. During detached wait, connect installs its own signal_hook handler so R11 cleanup runs (default disposition would skip Drop).
- **D7:** Command names are `zerdr detach` / `zerdr attach`. `zerdr connect` is not reused — it keeps its single role of creating a thread's first connection from inside the thread.
- **D8:** Workspace focus (`focus_workspace`) is deferred to the first successful attach: a connect that starts in detached wait (R3) must not move the shared Herdr session focus while the user is working from the phone.
- **D9:** User-facing strings are English (public repository).
- **D10:** Stale leases (dead connect, lock acquirable) found by the command scans are removed together with any orphan `.detached` marker, extending the existing `leased_panes` cleanup convention.

### Contracts

- CLI (public):
  - `zerdr detach` — suspend all thread attaches. Exit 0 on confirmed success or no-op; non-zero on wait timeout.
  - `zerdr attach` — resume all thread attaches. Same exit semantics.
- Filesystem (internal state, but relied on across processes):
  - Flag: `thread_detach_flag_file` exists ⇔ detach mode on. Created by `detach`, removed by `attach`.
  - Marker: `<lease dir>/<hash>.detached` exists ⇔ that connect's attach child is currently stopped because of detach mode. Written only by the lease holder; removed on reattach and on connect exit (guard cleanup).
  - Live lease = exclusive flock on `<hash>.json` is held (WouldBlock on probe). Scans treat lockable records as stale and delete them (+ orphan marker).
- Detached sidebar title forms (exact): `[herdr⏸] {label}` and `{glyph} [herdr⏸] {kind} - {detail}`.
- Thread terminal notices (exact wording finalized in implementation, must contain the command name): on detach `zerdr: detached from Herdr; run \`zerdr attach\` to reconnect`; on R5 `zerdr: Herdr pane {pane_id} is gone; closing this thread connection`.
- Connect lifecycle state machine: `Attached(child)` —flag set→ SIGTERM child, mark, `Detached` —flag cleared→ resolve terminal, spawn, unmark, `Attached(child)`. Child exiting on its own in `Attached` keeps today's exit contract (R12). Signals in `Detached` exit the loop gracefully (R11).

## Current Context

### Confirmed

- Herdr 0.8.2 behavior (measured in this repo's dev environment): direct attach pins the shared pane PTY to the attach client's size; the pane reverts to the full-client layout when the attach client disconnects; SIGTERM to the attach client restores alt-screen/mouse/kitty-keyboard/bracketed-paste on its terminal.
- `src/thread.rs` `run_with_mode`: resolve target → lease (`ThreadLeaseSet::acquire`, flock + `ThreadLeaseGuard` with `path()` and Drop cleanup) → focus workspace → `print_status` → spawn attach child (`ManagedChild`) → `SignalForwarder` → `Monitor` (condvar poll, `DEFAULT_POLL_MS` = 1000, `ZERDR_THREAD_POLL_MS` override; emits `[herdr]` titles and a bell on working→settled) → `child.wait()`.
- `ManagedChild` already has `try_wait()`; `terminate()` sends SIGKILL. `nix::sys::signal::kill` and `signal_hook` are existing dependencies.
- `Herdr::pane_terminal_for(session, pane_id)` exists (`pane get` → `terminal_id`) and is already used by the reattach-by-memory path (`thread.rs:316,373`); the fake herdr stubs `pane get` with `term-<pane_id>` and an error branch.
- `src/lib.rs`: remote rejection exempts only `Setup { Doctor }`; `accepts_session` gates the generic `--session` error; `ZERDR_TEST_REMOTE_MARKERS` drives remote tests.
- `Paths` has the `thread_auto_flag_file` precedent for flag files; `ZERDR_TEST_ROOT` reroutes all paths in tests.
- Test rig: `tests/support/mod.rs` fake herdr logs every invocation to `ZERDR_TEST_LOG`; attach branches block on `ZERDR_TEST_ATTACH_RELEASE_FILE` and write their pid to `ZERDR_TEST_CHILD_PID_FILE`; monitor tests sequence `agent get` via `ZERDR_TEST_AGENT_GET_SEQ` + `wait_for_sequence`.
- AGENTS.md validation: focused integration test first, then `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-targets --all-features`.

### Assumptions

- The `⏸` marker renders acceptably in the Zed threads sidebar (same glyph class as status glyphs already forwarded). Trivial to swap for another marker string if not.
- 5 s default wait comfortably covers the ≤1 s connect poll interval plus attach spawn time.

## File Structure

- Modify: `src/cli.rs` — add `Command::Detach` and `Command::Attach` variants (doc comments become `--help` text).
- Modify: `src/lib.rs` — dispatch the new commands; add them to the remote-rejection exemption (R10); keep them out of `accepts_session` (R9).
- Create: `src/suspend.rs` — `detach()` / `attach()` command orchestration: flag handling, lease scan, wait-with-timeout, reporting.
- Modify: `src/state.rs` — `Paths::thread_detach_flag_file`; flag set/clear/check helpers; `ThreadLeaseGuard::mark_detached`/`clear_detached` (sidecar); a scan API for suspend (live leases + marker state, stale cleanup per D10).
- Modify: `src/thread.rs` — replace `child.wait()` with the attach/detach cycle loop; initial flag check (R3); deferred focus (D8); `Monitor` detached mode (shared flag → marker title, bell suppressed).
- Modify: `src/herdr.rs` — graceful SIGTERM terminate on `ManagedChild` (D4).
- Modify: `README.md` — document the commands and the small-client (phone) use case in the Terminal threads section.
- Test: `tests/state_and_bindings.rs` — flag + marker + scan unit coverage.
- Test: `tests/thread_flow.rs` — connect suspend/resume cycle and end-to-end command flows.
- Test: `tests/cli_contract.rs` — help output, `--session` rejection, remote exemption.
- Test (support): `tests/support/mod.rs` — only if a new fake branch or env knob is needed (pane-get error already exists).

## Testing Decisions

- **Test seam:** the real `zerdr` binary and library APIs against the fake `herdr` under `TestEnv` (`ZERDR_TEST_ROOT`), as in `thread_flow.rs`. Detach state is observed through process liveness (attach pid), the `ZERDR_TEST_LOG` invocation log, flag/marker files, and connect stdout (OSC titles, notices).
- **Behavior:** drive a live connect (attach blocked on the release file), run `zerdr detach`/`zerdr attach` as separate processes, assert child termination, marker/flag lifecycle, reattach invocations (`pane get` + `terminal attach`), titles, bells, and exit codes.
- **Prior art:** `Fixture` in `thread_flow.rs` (child pid file, release file, `wait_for_sequence` for monitor timing); `ZERDR_TEST_REMOTE_MARKERS` in `cli_contract.rs`/`setup_and_doctor.rs` for remote rejection.
- **Avoid:** fixed sleeps racing the poll loop (use pid/marker/file waits with deadlines); asserting on private module internals; per-invocation setup in the shared PATH fake (bake per-test config via `baked_herdr` if needed).

## Progress

- [x] Task 1: State layer — detach flag, lease markers, suspend scan
- [x] Task 2: Connect suspend/resume cycle
- [x] Task 3: `zerdr detach` / `zerdr attach` commands
- [ ] Task 4: README documentation

## Tasks

### Task 1: State layer — detach flag, lease markers, suspend scan

**Covers:** D1, D2, D10 (foundation for R1–R4, R8)

**Objective:** zerdr state exposes the global detach flag, per-lease detach markers, and a scan that classifies live vs stale leases with their marker state, all covered by tests.

**Files:**
- Modify: `src/state.rs`
- Test: `tests/state_and_bindings.rs`

**Dependencies:** なし

**Implementation notes:**
- `Paths`: add `thread_detach_flag_file` in `from_roots` beside `thread_auto_flag_file`; flows through `for_test` automatically via `from_roots`.
- Flag helpers (set = create file, clear = remove, check = exists) following the `thread_auto_enabled` style; creation must `create_dir_all` the parent.
- `ThreadLeaseGuard`: derive the marker path from the lease path (`.json` → `.detached`); `mark_detached()` creates it, `clear_detached()` removes it, Drop removes it alongside the lease file.
- Scan API on `ThreadLeaseSet` for suspend: walk all scope directories under the root (all sessions), probe each `.json` with `try_lock_exclusive`; lockable → stale, delete record + orphan marker (extend the `leased_panes` convention); locked → live, report whether its marker exists. Return enough for counting and waiting (e.g. live total + detached total).
- Keep lease JSON schema untouched (no `schema_version` bump needed).

**Test cases:**
- Set/clear/check flag round-trip; check on a fresh test root → false.
- Guard `mark_detached` → marker file exists; `clear_detached` → gone; dropping the guard removes lease and marker.
- Scan with one held lease without marker → 1 live / 0 detached; with marker → 1/1.
- Scan with a stale record (file written, no lock) plus orphan marker → both files removed, not counted.

**Complete when:**
- New helpers compile with tests passing and existing state tests unchanged.

**Validation:**
- Run: `cargo test --test state_and_bindings`
- Expected: all tests pass, including the new flag/marker/scan cases.

**Result:** Done. `thread_detach_active/set/clear` are free functions in `state.rs` (flag `state_dir/thread-detach`); `ThreadLeaseGuard::mark_detached/clear_detached` manage the `<hash>.detached` sidecar and Drop removes it before the lease file; `ThreadLeaseSet::scan_all` probes locks only (no JSON parsing, so it is session-agnostic) and returns `ThreadLeaseScan { live, detached }`. Validation: `cargo test --test state_and_bindings` → 25 passed (4 new).

### Task 2: Connect suspend/resume cycle

**Covers:** R3, R5, R6, R7, R11, R12, D3, D4, D5, D6, D8

**Objective:** `zerdr connect` reacts to the detach flag: terminates and re-spawns its attach child across detach/attach transitions, reflects the state in titles and notices, and preserves today's behavior when the flag never appears.

**Files:**
- Modify: `src/thread.rs`
- Modify: `src/herdr.rs`
- Test: `tests/thread_flow.rs`

**Dependencies:** Task 1

**Implementation notes:**
- `ManagedChild`: add a graceful terminate (SIGTERM via `nix::sys::signal::kill`, then `wait()`); keep `terminate()`/Drop as SIGKILL backstop (D4).
- Replace the single `child.wait()` in `run_with_mode` with the state-machine loop from Contracts. Poll `try_wait` + flag at ≤1 s (reuse the `ZERDR_THREAD_POLL_MS` mechanism so tests can tighten it). Natural child exit in `Attached` keeps the existing exit-status contract (R12).
- On flag set: graceful-terminate child, drop the `SignalForwarder`, print the detach notice (after the Herdr client's own terminal restore output), `mark_detached()`.
- In `Detached`: register a signal_hook handler for SIGINT/SIGTERM/SIGHUP that breaks the loop so guards drop (R11). On flag cleared: `pane_terminal_for` → `spawn_terminal_attach_for`, new `SignalForwarder`, `clear_detached()`. On resolution failure: print the R5 notice, exit `Ok(())`.
- R3 start-up path: check the flag after lease acquisition and `print_status`; when set, print the detached notice, `mark_detached()`, skip `focus_workspace` (D8 — run it before the first real attach instead), and enter `Detached` directly.
- `Monitor`: share a detached flag (e.g. `Arc<AtomicBool>` owned by the loop); when set, `emit_title` renders `[herdr⏸]` in both forms and the settled bell is suppressed. Status tracking continues so titles stay live (R7).

**Test cases:**
- Flag set while attached (poll interval tightened) → attach child pid exits; connect stays alive; marker exists; stdout contains the detach notice and a `[herdr⏸]` OSC title.
- Detached + `agent get` sequence driving working→idle → no `\x07` in output; title still updates with the marker.
- Flag cleared → `ZERDR_TEST_LOG` shows `pane get` then `terminal attach term-<pane>`; marker removed; title marker reverts to `[herdr]`; releasing the release-file then ends connect with status 0 (existing contract).
- Flag cleared with fake `pane get` failing → connect prints the pane-gone notice and exits 0; lease and marker removed.
- Connect started with flag already set → no attach invocation in the log; no `workspace focus` invocation; detach notice printed; subsequent flag clear attaches and focuses.
- SIGINT to a detached connect → process exits; lease and marker files removed. (SIGTERM/SIGHUP share the same handler; one signal case suffices.)
- No-flag run (existing tests) → unchanged behavior.

**Complete when:**
- All new cases pass and every pre-existing `thread_flow` test passes unmodified (R12 regression gate).

**Validation:**
- Run: `cargo test --test thread_flow`
- Expected: full pass, no flaky timing (waits use files/pids, not fixed sleeps).

**Result:** Done. `attach_cycle` in `thread.rs` implements the state machine; `ManagedChild::terminate_gracefully` (SIGTERM) added in `herdr.rs`. Two deviations from the notes, both behavior-preserving: (1) the cycle polls on its own `ZERDR_THREAD_CYCLE_POLL_MS` (default 50 ms) instead of reusing `ZERDR_THREAD_POLL_MS` — the existing `detaching_does_not_wait_for_the_next_poll` test sets the monitor poll to 30 s and requires child exit to end connect promptly, so the two intervals must be independent; (2) `tests/support/mod.rs` fake `pane get` had a latent bug injecting `terminal_id` outside the pane object (real Herdr reports it inside `result.pane`) — fixed, which is what the first reattach exercised. Validation: `cargo test --test thread_flow` → 55 passed (5 new).

### Task 3: `zerdr detach` / `zerdr attach` commands

**Covers:** R1, R2, R4, R8, R9, R10, D1, D5, D7

**Objective:** The two public commands exist, orchestrate the flag + wait + report flow against live connects, and honor the session/remote contracts.

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/lib.rs`
- Create: `src/suspend.rs`
- Test: `tests/cli_contract.rs`
- Test: `tests/thread_flow.rs`

**Dependencies:** Task 1 (state API), Task 2 (connect reacts; end-to-end tests)

**Implementation notes:**
- `detach()`: set the flag, then poll the Task 1 scan until every live lease has its marker; deadline = 5 s default, `ZERDR_DETACH_WAIT_MS` override. Success: `zerdr: detached {n} thread(s)` (0 is fine — mention that new threads will start detached). Timeout: warn with the unconfirmed count on stderr, exit non-zero (`Error::User`).
- `attach()`: if the flag is absent and no live lease has a marker → `zerdr: detach mode is not active`, exit 0 (R8). Otherwise clear the flag, poll until no live lease has a marker, report `zerdr: reattached {n} thread(s)` counting the markers cleared; same timeout semantics. A connect that exits during the wait (R5 pane-gone) releases its lease and must not stall the wait.
- `lib.rs`: dispatch the variants; add them to the remote-rejection exemption match alongside `Setup { Doctor }` (R10); do not add them to `accepts_session` (R9 comes free from the existing generic error).
- Keep all scanning/waiting logic in `src/suspend.rs`; `lib.rs` stays a thin dispatcher per repo convention.

**Test cases:**
- `zerdr detach --session x` / `zerdr attach --session x` → exit non-zero with the existing `--session cannot be used with this command` message.
- `zerdr --help` lists both commands; `cli_contract` help assertions (substring `predicate::str::contains`, no snapshots in this repo) updated.
- Remote markers set → `detach`/`attach` still run (no rejection); `connect` still rejects (regression). Cover both marker kinds like `tests/cli_contract.rs:249-302`: env markers (e.g. `SSH_CONNECTION` set directly) and file markers (`ZERDR_TEST_REMOTE_MARKERS`, which fakes only the filesystem markers).
- End-to-end (thread_flow): live connect → `zerdr detach` exits 0 printing `detached 1 thread(s)`, attach pid dead → `zerdr attach` exits 0 printing `reattached 1 thread(s)`, log shows the reattach invocations.
- `zerdr detach` with no live threads → exit 0, flag set, message notes new threads start detached; `zerdr attach` right after → exit 0, flag cleared.
- `zerdr attach` when never detached → no-op message, exit 0.
- Timeout path: flag set but a live lease never marks (e.g. connect with poll interval set huge) → `zerdr detach` with `ZERDR_DETACH_WAIT_MS` small exits non-zero and warns.

**Complete when:**
- Both commands behave per R1/R2/R4/R8 in the end-to-end tests and the CLI contract tests pass.

**Validation:**
- Run: `cargo test --test cli_contract --test thread_flow`
- Expected: full pass including the new end-to-end flows.

**Result:** Done. `src/suspend.rs` holds both commands (flag + `scan_all` poll at 50 ms, 5 s budget via `ZERDR_DETACH_WAIT_MS`); timeout reports the pending count and exits non-zero. Messages: `detached N thread(s)` / `no live threads; new threads will start detached` / `reattached N thread(s)` / `detach mode is off; no threads were waiting` / `detach mode is not active`. The timeout e2e test tears its connect down with SIGKILL instead of waiting out the deliberately huge cycle poll. Validation: `cargo test --test cli_contract --test thread_flow` → 23 + 59 passed.

### Task 4: README documentation

**Covers:** D7, D9 (user-facing surface of R1–R4)

**Objective:** README's Terminal threads section documents when and how to use `zerdr detach` / `zerdr attach`.

**Files:**
- Modify: `README.md`

**Dependencies:** Task 3 (final command names/messages)

**Implementation notes:**
- Brief English addition: attached threads pin their pane's size; before opening the session from a small client (e.g. a phone over SSH), run `zerdr detach`; run `zerdr attach` to reconnect every thread afterwards; both work over SSH on the same machine. Keep README user-focused per AGENTS.md (no internals).

**Test cases:**
- N/A (prose). Reviewed against actual `--help` output for consistency.

**Complete when:**
- Section added; command names and semantics match the implementation.

**Validation:**
- Run: `cargo run --locked -- --help`
- Expected: help output lists `detach` and `attach` matching the README wording.

## Requirement Coverage

| Requirement / Decision | Task | Verification |
|---|---|---|
| R1 | Task 3 | e2e: detach kills attach pid, connect alive, lease held |
| R2 | Task 3 | e2e success message + timeout test (non-zero, warning) |
| R3 | Task 2 | connect-under-flag test: no attach/focus invocation, notice printed |
| R4 | Task 3 | e2e: attach clears flag, log shows `pane get` + `terminal attach` |
| R5 | Task 2 | pane-get-failure test: notice, exit 0, lease/marker removed |
| R6 | Task 2 | attach child terminated via SIGTERM path; detach notice in stdout (mode restore verified upstream, recorded in Confirmed) |
| R7 | Task 2 | `[herdr⏸]` OSC title asserted; no `\x07` during detached working→idle |
| R8 | Task 3 | no-op attach test; repeat-detach covered by 0-thread test |
| R9 | Task 3 | cli_contract `--session` rejection tests |
| R10 | Task 3 | remote-markers test: detach/attach run, connect rejects |
| R11 | Task 2 | SIGINT-while-detached test: exit + lease/marker cleanup |
| R12 | Task 2 | pre-existing thread_flow tests pass unmodified |
| D1, D2, D10 | Task 1 | flag/marker/scan unit tests incl. stale cleanup |
| D3, D4, D5, D6, D8 | Task 2 | reattach-by-terminal, SIGTERM path, poll override, signal handling, deferred-focus tests |
| D7 | Task 3, 4 | command names in CLI contract tests and README |
| D9 | Task 3, 4 | messages asserted in tests are English |

## Final Validation

- [ ] `cargo test --test state_and_bindings --test thread_flow --test cli_contract` — Expected: pass
- [ ] `cargo test --all-targets --all-features` — Expected: pass
- [ ] `cargo fmt --all -- --check` — Expected: no diff
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` — Expected: no warnings
- [ ] Manual check in the real environment: with a Zed thread attached, run `zerdr detach`, confirm the pane reflows for the Herdr desktop client (and a small client if available), thread title shows `[herdr⏸]`; run `zerdr attach`, confirm the thread reconnects and the title marker reverts
- [ ] Requirement Coverage に未対応項目がない
- [ ] 計画と実際の変更内容が整合している
- [ ] 上記のすべてが成功した後、計画を同名のまま `docs/plans/archived/` へ移した

## Risks and Open Questions

- Timing-sensitive integration tests: the cycle loop and command waits are file/pid-driven; keep every wait deadline-based (AGENTS.md warns fixed sleeps fail under parallel load).
- Terminal residue after SIGTERM: the Herdr client restores modes, but the thread keeps the last rendered frame; the detach notice prints after it. Cosmetic only.
- If Zed kills the thread terminal while detached (window closed), the connect dies without running Drop for the signal-less kill path (SIGKILL); stale lease + orphan marker are cleaned by the next command scan (D10) — verify the scan handles this in Task 1 tests.
- 未解決事項: なし
