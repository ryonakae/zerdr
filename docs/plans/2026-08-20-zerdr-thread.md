# `zerdr thread` (Zed Terminal Thread Bridge) Implementation Plan

> **For implementers:** Execute tasks in order unless dependencies allow otherwise. Mark a task complete only after its validation succeeds. Reflect minor implementation differences in the relevant task. Ask the user before changing requirements, Out of Scope, or public contracts.

## Problem Statement

Zed's Agent Panel hosts Terminal Threads whose sidebar entries take their titles from terminal title updates (OSC) and surface notifications from bell characters. Herdr keeps agents alive in panes across editor restarts and is reachable over SSH from mobile. Today there is no bridge: running `herdr agent attach` inside a Terminal Thread works, but the sidebar title never reflects the agent, no notification fires when the agent finishes, and every attach target must be looked up and typed by hand.

A prototype (fish script: background poll of `herdr agent get` + OSC title injection + BEL injection + foreground `herdr agent attach`) was validated end to end in the user's real Zed + Herdr environment on 2026-08-20:

- injected OSC titles appear and persist in the Zed Threads Sidebar;
- injected BEL raises a Zed notification when the thread is unfocused (requires `agent.notify_when_agent_waiting` / `play_sound_when_agent_done`);
- multiple Terminal Threads can attach to different panes of the same Herdr session concurrently;
- Terminal Threads survive Zed project switches (window-scoped, attach clients keep running).

This plan productizes that prototype as a `zerdr thread` subcommand.

## Goal

`zerdr thread` runs inside a Zed Terminal Thread (configured as Zed's `agent.terminal_init_command` by `zerdr setup`) and:

- attaches to a Herdr agent pane (explicit target, or auto-resolved from the Zed project's Git root);
- mirrors the agent's identity and Herdr terminal title into the Zed sidebar via OSC;
- injects BEL on `working → idle/done/blocked` transitions so Zed notifies;
- starts a new agent (default kind `pi`) in a new tab when the matching workspace has no unattached agent, so "new Terminal Thread = new agent";
- creates the workspace itself only on explicit `--create` (bare invocations in projects Herdr does not manage fail with guidance instead of growing the workspace list);
- focuses the matching Herdr workspace on start (Zed → Herdr workspace sync).

## Out of Scope

- Forking or patching Zed; anything requiring Zed-side code (live sidebar auto-add when agents appear in Herdr, Herdr → Zed thread selection sync, external in-window project switching via `zed -r`).
- An ACP adapter or any Agent Client Protocol work.
- A Herdr socket JSON-RPC client (`events.subscribe`); v1 monitors by CLI polling only.
- Syncing Zed terminal tab/pane structure with Herdr tabs/panes.
- Changes to the Herdr plugin manifest, follow-mode wrapper, routing, or `sync`/`pick`/`next`/`previous` behavior.
- Mobile/SSH behavior changes (Herdr already covers this).
- Starting or supervising the Herdr server when the target session is not running.

## Requirements and Decisions

### Requirements

- **R1:** `zerdr thread [TARGET]` attaches to a Herdr agent pane by spawning `herdr [--session NAME] agent attach <pane_id>` with inherited stdio, forwards SIGINT/SIGTERM/SIGHUP to the child (same pattern as `run_wrapper`), and exits with the child's status.
- **R2:** With explicit `TARGET` (pane ID such as `w1:p1`, or unique live agent name), zerdr validates it via `herdr agent get <TARGET>` and attaches to the resolved pane. Explicit targets still get title/bell monitoring and a thread lease, but skip auto-start.
- **R3:** Bare `zerdr thread` resolves the Zed project: cwd → `canonical_git_root` → the session workspace whose `BindingStore` entry or `worktree.checkout_path` equals that root. Among that workspace's live agents (`herdr agent list` filtered by `workspace_id`), it attaches to the first pane without a live thread lease.
- **R4:** If the matched workspace has no unleased agent pane, zerdr creates one: `herdr tab create --workspace <id> --cwd <root> --no-focus`, then `herdr agent start <generated-name> --kind <kind> --pane <root_pane_id>`, then attaches. Kind comes from `--kind`, else `ZERDR_THREAD_KIND`, else `pi`. Generated names are `zed-<n>` (lowest `n` not colliding with live agent names; see Confirmed for the herdr name rules).
- **R5:** If no workspace matches the root, bare `zerdr thread` exits with a one-line error naming the root and advising `zerdr thread --create` (user decision 2026-08-20: no silent workspace creation, because `terminal_init_command` is a Zed-global setting and would otherwise grow Herdr's workspace list for every Git project opened in Zed). With `--create`, zerdr creates the workspace (`herdr workspace create --cwd <root> --label <repo dir name> --no-focus`), records the binding via `BindingStore::bind_if_absent`, and starts the agent in the returned root pane per R4.
- **R6:** While attached, a monitor polls `herdr agent get <pane_id>` every `ZERDR_THREAD_POLL_MS` ms (default 2000) and writes an OSC 0 title `\x1b]0;{agent} · {terminal_title_stripped}\x07` to zerdr's stdout whenever the computed label changes. Empty `terminal_title_stripped` falls back to the workspace label; a missing agent leaves the last title in place.
- **R7:** The monitor writes BEL (`\x07`) to stdout exactly when `agent_status` transitions from `working` to one of `idle`, `done`, `blocked`.
- **R8:** Monitor failures (herdr CLI error, JSON error, pane gone) never terminate the attach child; the monitor stops when the child exits. Polling errors are silent (no stderr spam).
- **R9:** A per-pane thread lease (keyed by session name + socket + pane ID) prevents two bare invocations from picking the same pane. Liveness follows the existing `LeaseSet` mechanism: an advisory `flock` held open by the owning process, so a lease whose lock is no longer held is treated as free and cleaned up. The lease is released on normal exit and on signal-driven teardown (guard Drop). Resolution → creation → lease acquisition for bare invocations is serialized per session socket (an `OperationGuard`-style lock) so two racing bare invocations on a workspace with no agent panes cannot both create a tab and start an agent.
- **R10:** On start, after resolving the pane, if the pane's workspace is not the focused workspace, zerdr runs `herdr workspace focus <workspace_id>`. Focus failure is non-fatal (single warning line to stderr, then continue).
- **R11:** `zerdr setup` sets `agent.terminal_init_command` in Zed's `settings.json` (same config dir already used for `tasks.json`) to `"<canonical zerdr executable> thread"`, but only when the key is absent or its current value matches a zerdr-owned fingerprint recorded in `InstallState`. A foreign value is never overwritten; setup prints a notice instead. `zerdr uninstall` removes the key only when it matches the owned value. JSONC editing uses the existing `jsonc-parser` CST approach so user comments/formatting survive.
- **R12:** `zerdr doctor` reports terminal_init_command status: `ok` (owned and current), `missing`, or `foreign` (present but not zerdr-owned), with remediation hints.
- **R13:** `--session NAME` selects the Herdr session (default `default`), following the existing "specify `--session` only once" CLI rules. `--kind` or `--create` combined with an explicit `TARGET` is a usage error declared in clap (`conflicts_with`), so it is caught at parse time and exits 2; it must not be implemented as a post-parse `Error::User` check (which would exit 1). Remote environments are rejected exactly like other commands (existing `runtime::detect_remote_environment` gate in `run()` already covers new subcommands).
- **R14:** When the target session is not running, cwd is not inside a Git checkout on the bare path, or no workspace matches without `--create` (R5), zerdr exits with a one-line actionable error (the surrounding Zed shell remains usable). It must not spawn a Herdr server.
- **R15:** `zerdr thread` must work without the follow-mode wrapper running and without `validate_launcher_installation` passing (it depends on Herdr only, not on the plugin/tasks install).

### Implementation Decisions

- **D1:** Monitor by CLI polling (`herdr agent get`), not socket `events.subscribe`. Rationale: proven by the prototype; the public 0.8.2 event schema has no title-change event, so polling is needed for titles regardless; avoids writing an NDJSON socket client in v1.
- **D2:** Default auto-start kind is `pi` (user decision 2026-08-20). "New Terminal Thread = new agent" is the intended UX within an existing workspace; erroring out there was rejected. Workspace creation is the opposite: explicit `--create` only (user decision 2026-08-20).
- **D3:** OSC/BEL go to zerdr's own stdout (the same pty Zed reads), not `/dev/tty`. This keeps the escape bytes capturable in integration tests. Writes are single short `write` + flush to minimize interleaving with attach output.
- **D4:** The thread lease store lives in `state.rs` beside `LeaseSet`, reusing its flock-based liveness (an advisory lock held open by the owner; staleness = lock no longer held, not a stored-pid check) and hashed-scope file layout, with a new `Paths` directory (e.g. `thread_leases_dir`). It is a separate type, not a `LeaseSet` extension, because the key includes a pane ID and semantics differ (many live leases per session).
- **D5:** Settings ownership mirrors the `task_fingerprints` pattern: `InstallState` gains an optional fingerprint for the init command value; absence in old state files must deserialize cleanly (backward compatible).
- **D6:** Workspace focus only fires when the workspace is not already focused, to avoid re-triggering the `workspace.focused` plugin event chain (`sync-from-herdr` → Zed activation) on every thread start.
- **D7:** Orchestration goes in a new `src/thread.rs`; `herdr.rs` gains only thin typed wrappers over the CLI; `state.rs` gains only the lease store. No changes to `sync.rs` routing.

### Contracts

CLI:

```
zerdr thread [TARGET] [--session <NAME>] [--kind <KIND>] [--create]
```

- `TARGET`: Herdr pane ID or unique live agent name (optional).
- `--kind`: agent kind for auto-start (default `pi`); clap `conflicts_with` `TARGET`.
- `--create`: allow creating the Herdr workspace when none matches the Git root; clap `conflicts_with` `TARGET`.
- Environment: `ZERDR_THREAD_KIND`, `ZERDR_THREAD_POLL_MS`, existing `ZERDR_HERDR_BIN`.
- Exit status: attach child's status on success path; existing `error.rs` codes otherwise; clap usage errors exit 2.

Terminal output contract (consumed by Zed):

- Title: `\x1b]0;{agent} · {title}\x07`, emitted only on change.
- Notification: `\x07`, emitted only on `working → idle|done|blocked`.

Herdr CLI surface used (verified against herdr 0.8.2): `agent list`, `agent get <target>`, `agent attach <target>`, `agent start <name> --kind <kind> --pane <id>`, `tab create --workspace <id> --cwd <path> --no-focus [--label]`, `workspace create --cwd <path> --label <text> --no-focus`, `workspace list`, `workspace focus <id>`, `session list --json`.

Zed settings contract: `agent.terminal_init_command` (string) in the user `settings.json`; runs on Terminal Thread creation and on recreation after reopening a project.

## Current Context

### Confirmed

- Prototype behavior validated in the real environment (see Problem Statement).
- `herdr agent get` returns `{"result":{"agent":{agent, agent_status, pane_id, workspace_id, terminal_title_stripped, ...}}}`; `agent list` returns the same objects under `result.agents`; statuses observed: `idle`, `working` (schema also defines `blocked`, `done`, `unknown`).
- `herdr agent start` requires an existing pane at a shell prompt and a unique name; supported kinds include `pi` and `claude`. Live agent names must match `[a-z][a-z0-9_-]{0,31}` and be unique among live agents (per the `herdr --skill` documentation shipped with herdr 0.8.2).
- `herdr workspace create` / `tab create` accept `--cwd`, `--label`, `--no-focus`; creation responses expose `.result.workspace`, `.result.tab`, `.result.root_pane`.
- `zerdr` already has: `Herdr` CLI wrapper with `--session` plumbing and JSON parsing helpers (`src/herdr.rs`), `BindingStore` + `canonical_git_root` + pid-liveness lease patterns (`src/state.rs`), signal forwarding + managed child (`run_wrapper` in `src/herdr.rs`), JSONC CST merging with ownership fingerprints for Zed `tasks.json` (`src/setup.rs`), fake `herdr`/`zed` script harness driven by `ZERDR_TEST_*` env vars (`tests/support/mod.rs`).
- Zed sidebar titles/notifications are driven by OSC title updates and BEL; `notify_when_agent_waiting` / `play_sound_when_agent_done` must be enabled by the user for notifications.

### Assumptions

- Zed runs `terminal_init_command` inside the thread's shell, so a failing `zerdr thread` leaves a usable shell showing the error (observed indirectly; if Zed instead replaces the shell, only the error-message UX changes, not the contract).
- `herdr agent attach` without `--takeover` supports concurrent clients on *different* panes (validated) and behaves acceptably if the user manually double-attaches one pane (leases prevent this only for bare invocations).

## File Structure

- Create: `src/thread.rs` — `zerdr thread` orchestration: target resolution, workspace/tab/agent auto-creation, thread lease, workspace focus guard, attach child + monitor loop.
- Modify: `src/cli.rs` — `Thread { target, session, kind }` subcommand.
- Modify: `src/lib.rs` — dispatch + `--session` once-only accounting for the new subcommand.
- Modify: `src/herdr.rs` — typed helpers: `agents_for`, `agent_get_for`, `spawn_agent_attach_for`, `tab_create_for`, `workspace_create_for`, `agent_start_for`.
- Modify: `src/state.rs` — `ThreadLeaseSet` + `Paths.thread_leases_dir`.
- Modify: `src/setup.rs` — `agent.terminal_init_command` merge/removal + `InstallState` fingerprint field.
- Modify: `src/doctor.rs` — init-command status check.
- Test: `tests/thread_flow.rs` — end-to-end thread behavior against the fake herdr.
- Test: `tests/support/mod.rs` — extend fake `herdr` with `agent list/get/attach/start`, `tab create`, `workspace create` (responses via `ZERDR_TEST_*` env vars; attach blocks until a release file, logging its args).
- Test: `tests/setup_and_doctor.rs` — settings merge/removal/doctor cases.
- Test: `tests/cli_contract.rs` — flag validation for the new subcommand.

## Testing Decisions

- **Test seam:** the compiled `zerdr` binary invoked with fake `herdr`/`zed` scripts (existing `tests/support` pattern). Assert on the fake-herdr invocation log, zerdr's captured stdout bytes (OSC/BEL), lease files on disk, and exit codes.
- **Behavior:** fake `agent get` responses are scripted per poll (sequence file advanced by the fake), so title changes and status transitions are deterministic; fake `agent attach` blocks until a test-controlled release file exists, then exits with a configurable code.
- **Prior art:** `tests/sync_flow.rs` (wrapper lifecycle, markers, lease assertions), `tests/setup_and_doctor.rs` (JSONC merge fixtures).
- **Avoid:** asserting on poll timing (drive transitions by response sequences, use a short `ZERDR_THREAD_POLL_MS`), parsing internal lease file formats beyond existence/liveness, and any test requiring a real pty or real Herdr.

## Progress

- [x] Task 1: Herdr CLI helpers + fake herdr extensions
- [ ] Task 2: Thread lease store in `state.rs`
- [ ] Task 3: `zerdr thread` subcommand (resolution, auto-start, attach, monitor)
- [ ] Task 4: Setup/uninstall/doctor integration for `terminal_init_command`
- [ ] Task 5: Documentation and manual validation

## Tasks

### Task 1: Herdr CLI helpers + fake herdr extensions

**Covers:** R1 (spawn), R2–R5 (data access), Contracts (Herdr CLI surface)

**Objective:** `Herdr` exposes typed helpers for every Herdr call the thread flow needs, and the fake herdr can serve them deterministically.

**Files:**
- Modify: `src/herdr.rs`
- Test: `tests/support/mod.rs`, `tests/herdr_wrapper.rs`

**Dependencies:** none.

**Implementation notes:**
- Follow the existing `session_json_output_for` / `string_field` parsing style; return small structs (`AgentInfo { name_or_kind: agent, status, pane_id, workspace_id, title }`, creation results carrying `root_pane` pane ID).
- `spawn_agent_attach_for(session, target)` mirrors `spawn_client` (inherited stdio, `--session` only for non-default) and returns `Child`.
- Fake herdr: `agent list`/`agent get` echo `ZERDR_TEST_AGENTS_JSON` / a sequence directory (`ZERDR_TEST_AGENT_GET_SEQ` of numbered responses, last one repeating, a leading `EXIT:<code>` line simulating a failed poll); `agent attach` logs args, waits for `ZERDR_TEST_ATTACH_RELEASE_FILE`, exits `ZERDR_TEST_ATTACH_EXIT` (default 0); `agent start`, `tab create`, `workspace create` log args and print JSON from env vars; when `ZERDR_TEST_AGENTS_DIR` is configured, `agent start` writes the started agent into it and `agent list` assembles the directory, so Task 3's race test sees agents created by concurrent invocations.

**Implementation record:**
- Added `Herdr::with_program` so library-level tests can point the adapter at a fake script without env mutation. Helpers landed as `agents_for`, `agent_get_for` (returns `Option<AgentInfo>`; a response without an agent is not an error, per R6), `agent_start_for`, `tab_create_for`, `workspace_create_for`, `spawn_agent_attach_for`, plus `AgentInfo`/`CreatedWorkspace`.
- `spawn_agent_attach_for` always passes `--session NAME` (including `default`), matching every other session-scoped call in `herdr.rs`, rather than omitting it like `spawn_client`.
- The fake-herdr dispatch body moved into a `FAKE_HERDR_BODY` const shared by the `PATH` fake and a new `TestEnv::baked_herdr(name, variables)`, which writes a private script with the `ZERDR_TEST_*` values baked in. Reason: an earlier attempt sourced a config file from the shared fake on every invocation, and that single added line pushed `doctor_waits_for_admission_and_preserves_the_new_live_route` (a 2-second wrapper timing budget) over its limit under parallel load. The shared fake is now byte-identical to its previous content.

**Test cases:**
- `agents_for` parses the real-shape `agent_list` JSON (fixture copied from a live capture) → correct pane/workspace/status/title fields.
- `agent_get_for` on error-status fake (exit 1, JSON on stderr) → `Error::Process`, not a panic.
- `tab_create_for` / `workspace_create_for` surface `root_pane.pane_id` from `.result`.

**Complete when:**
- All new helpers have at least one passing test through the fake herdr.
- Existing `tests/herdr_wrapper.rs` cases still pass.

**Validation:**
- Run: `cargo test --test herdr_wrapper`
- Expected: all tests pass, including new helper cases.

### Task 2: Thread lease store in `state.rs`

**Covers:** R9, D4

**Objective:** `ThreadLeaseSet` can acquire, inspect, and release per-pane leases with pid liveness, isolated from the existing wrapper `LeaseSet`.

**Files:**
- Modify: `src/state.rs` (`ThreadLeaseSet`, `Paths.thread_leases_dir`)
- Test: `tests/state_and_bindings.rs`

**Dependencies:** none.

**Implementation notes:**
- Key: hash of (session name, canonical socket path, pane ID); file stores pane ID + pid + acquired-at metadata, written with `atomic_write_json`; liveness comes from the advisory `flock` the owner keeps open (same mechanism as `LeaseSet`), not from the stored pid.
- `acquire` fails if the key's lock is currently held; a lease file whose lock is not held is stale and replaced. Provide `leased_panes(session, socket) -> BTreeSet<String>` for R3 filtering (locked keys only). Guard releases on Drop.

**Test cases:**
- Acquire → file exists; second acquire same pane while the first guard lives → error; different pane → ok.
- Stale lease (file written without a held lock, e.g. left over from a killed process) → `leased_panes` excludes it and re-acquire succeeds.
- Drop guard → subsequent acquire succeeds and `leased_panes` no longer lists the pane.

**Complete when:**
- Above cases pass; existing state tests unaffected.

**Validation:**
- Run: `cargo test --test state_and_bindings`
- Expected: all pass.

### Task 3: `zerdr thread` subcommand

**Covers:** R1–R10, R13–R15, D1–D3, D6, D7

**Objective:** The full thread flow works end to end against the fake herdr: resolve → (create) → lease → focus-if-needed → attach → mirror titles/bells → propagate exit.

**Files:**
- Create: `src/thread.rs`
- Modify: `src/cli.rs`, `src/lib.rs`
- Test: `tests/thread_flow.rs`, `tests/cli_contract.rs`

**Dependencies:** Task 1, Task 2.

**Implementation notes:**
- Reuse `ManagedChild` + `SignalForwarder` from `herdr.rs` (make them `pub(crate)` if needed).
- Bare resolution order (R3): binding match first (`BindingStore::get` per workspace), then `worktree.checkout_path` equality against `canonical_git_root(cwd)`; do not call `validate_launcher_installation` (R15).
- Monitor runs on a thread; label per R6; keep last seen status across polls; stop via a flag checked each interval after the child exits (main loop uses `try_wait` + short sleep, mirroring `run_wrapper`).
- Stdout writes: single `write_all` of the full escape sequence + flush (D3).
- `--kind`/`--create` vs `TARGET` exclusivity is declared in clap (`conflicts_with`) so clap reports it and exits 2 before `run()`; do not re-implement it as `Error::User` (exit 1).
- Serialize the bare-invocation resolve → create → lease sequence per session socket with an `OperationGuard`-style lock (R9) so racing invocations cannot both create a tab/agent; the fake herdr must append started agents to its agent-list state so the second invocation sees the first's agent.
- Workspace focus (R10/D6): compare against `focused` from `workspace list` before issuing `workspace focus`; on failure print one `zerdr: ...` warning to stderr.
- R14 errors: no running session → mention starting Herdr (`herdr` or `zerdr`); no Git root → mention running from a Git checkout or passing an explicit TARGET; no matching workspace → name the root and mention `zerdr thread --create`.

**Test cases:**
- Existing unleased agent in bound workspace → fake log shows `agent attach <pane>`, no `tab create`/`agent start`; lease exists during attach and is gone after release-file exit.
- Second concurrent bare invocation (attach blocked) → attaches to the *other* agent pane.
- No agent panes in workspace → log shows `tab create --workspace ... --no-focus` then `agent start zed-1 --kind pi --pane <root_pane>` then `agent attach`.
- No matching workspace, bare → exit 1 with an error naming the root and `zerdr thread --create`; no `workspace create` in the log.
- No matching workspace, `--create` → `workspace create --cwd <root> ... --no-focus`, binding file gains the entry, `agent start` in returned root pane.
- Two concurrent bare invocations on a workspace with no agent panes (stateful fake) → the log contains exactly one `tab create` and one `agent start`; the second invocation attaches to the newly started agent's pane or waits on the serialization lock.
- `--kind claude` → `agent start ... --kind claude`; `ZERDR_THREAD_KIND=claude` (no flag) → same; `zerdr thread wM:p8 --kind pi` and `zerdr thread wM:p8 --create` → clap usage error, exit 2.
- Agent-get sequence: title A → title A (no dup) → title B; statuses working → working → idle. Captured stdout contains exactly two OSC title sequences and exactly one BEL.
- Empty `terminal_title_stripped` in a response → title falls back to `{agent} · {workspace label}`.
- Agent-get failure mid-sequence (fake exits 1 for one poll) → attach child unaffected, nothing written to stderr, monitoring resumes on the next poll; a response with no agent leaves the last emitted title in place (no new OSC).
- Fake attach exits 3 → zerdr exits 3.
- Workspace already focused → no `workspace focus` in log; unfocused → exactly one.
- Remote-environment env markers set → rejection error (same as other commands).
- Name collision: live agent named `zed-1` in fake data → auto-start uses `zed-2`.

**Complete when:**
- All above pass; `cargo test` full suite passes (no regression in sync/setup flows).

**Validation:**
- Run: `cargo test --test thread_flow --test cli_contract`
- Expected: all pass.

### Task 4: Setup/uninstall/doctor integration

**Covers:** R11, R12, D5

**Objective:** `zerdr setup` installs the init command safely, `zerdr uninstall` removes only what it owns, `zerdr doctor` reports the state.

**Files:**
- Modify: `src/setup.rs`, `src/doctor.rs`
- Test: `tests/setup_and_doctor.rs`

**Dependencies:** none (parallel to Tasks 1–3), but doctor wording should match Task 3's command name.

**Implementation notes:**
- Reuse the `tasks.json` JSONC CST merge machinery for `settings.json` in the same Zed config directory; create the file if absent.
- `InstallState` gains `#[serde(default)] terminal_init_command_fingerprint: Option<String>`; schema_version unchanged (additive, old files must load).
- Foreign-value notice must name the setting and the value zerdr would have written.

**Test cases:**
- No settings.json → created containing `"agent": { "terminal_init_command": "<exe> thread" }`; fingerprint recorded.
- settings.json with user comment and unrelated keys → key added, comment preserved.
- Existing foreign `terminal_init_command` → unchanged; setup succeeds with notice on stdout.
- Re-setup after executable path changes → owned value updated, fingerprint updated.
- Uninstall with owned value → key removed (file otherwise intact); with foreign value → untouched.
- Doctor: ok / missing / foreign each reported with distinct text.

**Complete when:**
- Above pass; existing setup/uninstall/doctor tests pass.

**Validation:**
- Run: `cargo test --test setup_and_doctor`
- Expected: all pass.

### Task 5: Documentation and manual validation

**Covers:** R6, R7, R10, R11 (real-environment confirmation); documentation of the new surface

**Objective:** README/AGENTS.md describe `zerdr thread`, and the flow is confirmed once in the real Zed + Herdr environment.

**Files:**
- Modify: `README.md` (usage section), `AGENTS.md` (module map: `src/thread.rs`)

**Dependencies:** Tasks 1–4.

**Implementation notes:**
- Document the Zed prerequisites: `agent.notify_when_agent_waiting`, `play_sound_when_agent_done`, and that `zerdr setup` sets `agent.terminal_init_command`.
- Manual checklist is the validation below; run it with the user (real Zed cannot be driven from tests).

**Test cases:**
- N/A (documentation + manual pass).

**Complete when:**
- Docs updated; manual checklist executed with all items confirmed by the user.

**Validation:**
- Manual, in Zed: (1) new Terminal Thread auto-runs `zerdr thread` and attaches to an existing agent with correct sidebar title; (2) second thread attaches to a different agent; (3) thread in a project with no agents starts `pi` in a new tab; (4) prompt the agent, unfocus, notification fires on completion; (5) switch project via picker and back — threads still attached; (6) Herdr workspace focus followed the Zed project on thread start.
- Expected: all six observed.

## Requirement Coverage

| Requirement / Decision | Task | Verification |
|---|---|---|
| R1 | Task 1, 3 | attach spawn test; exit-code propagation test |
| R2 | Task 3 | explicit-target test via `agent get` |
| R3 | Task 3 | bound-workspace attach + concurrent-invocation tests |
| R4 | Task 3 | tab create + `agent start --kind pi` test; name collision test |
| R5 | Task 3 | bare no-workspace error test; `--create` workspace create + binding test |
| R6 | Task 3 | OSC dedup sequence test; empty-title fallback test; missing-agent keeps-title test; Task 5 manual (1) |
| R7 | Task 3 | single-BEL transition test; Task 5 manual (4) |
| R8 | Task 3 | agent-get failure mid-sequence test (attach unaffected, silent, resumes) |
| R9 | Task 2, 3 | lease acquire/collision/stale-lock tests; lease-released-after-exit test; empty-workspace race test |
| R10 | Task 3 | focused/unfocused workspace-focus tests; Task 5 manual (6) |
| R11 | Task 4 | settings merge/foreign/uninstall tests |
| R12 | Task 4 | doctor ok/missing/foreign tests |
| R13 | Task 3 | cli_contract: `--session` rules; `--kind`/`--create`+TARGET → clap error, exit 2 |
| R14 | Task 3 | no-session and no-git-root error tests |
| R15 | Task 3 | thread flow tests run without plugin/install state present |
| D1 | Task 3 | monitor implemented via `agent get` polling (code review; poll-interval env respected in tests) |
| D2 | Task 3 | default-kind test asserts `pi` |
| D3 | Task 3 | OSC/BEL asserted on captured stdout |
| D4 | Task 2 | `ThreadLeaseSet` isolated from `LeaseSet` (existing lease tests unaffected) |
| D5 | Task 4 | old `InstallState` JSON without the new field still loads |
| D6 | Task 3 | already-focused → no focus call |
| D7 | Task 1–3 | file placement per File Structure (review) |

## Final Validation

- [ ] `cargo fmt --all -- --check` — Expected: no diffs.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` — Expected: no warnings.
- [ ] `cargo test --all-targets --all-features` — Expected: full suite passes.
- [ ] Manual checklist in Task 5 executed in real Zed + Herdr — Expected: all six items confirmed.
- [ ] Requirement Coverage has no unmapped rows.
- [ ] Plan matches the actual changes (update tasks for minor drift before closing).
- [ ] After all above succeed, move this file unchanged to `docs/plans/archived/`.

## Risks and Open Questions

- OSC/BEL writes can theoretically interleave with attach TUI redraw bytes; the prototype showed no corruption, and single flushed writes minimize the window. If corruption appears, buffer injections to occur only when the child has been quiet for a beat.
- `herdr agent attach --takeover` semantics are undocumented; v1 never passes it. Manual double-attach of one pane by explicit TARGET is allowed and unguarded (user intent).
- `workspace focus` on thread start triggers the `workspace.focused` plugin event; with follow-mode running this may activate Zed (focus steal). D6 limits this to genuinely unfocused workspaces; verify in Task 5 manual (6) and, if disruptive, gate R10 behind a flag after asking the user.
- Zed's exact execution model for `terminal_init_command` (command in shell vs. replacement) is unverified; affects only failure-mode UX (see Assumptions).
- Open questions: none blocking; all public contracts above are decided.
