# CLI Restructure and Named-Session Headless Start Implementation Plan

> **For implementers:** Execute tasks in order unless dependencies allow otherwise. Mark a task complete only after its validation succeeds. Reflect minor implementation differences in the relevant task. Ask the user before changing requirements, Out of Scope, or public contracts.

## Problem Statement

The CLI has grown to eight top-level commands (`zerdr`, `sync`, `bind`, `unbind`, `thread`, `setup`, `uninstall`, `doctor`) whose names do not communicate what happens when they run, and `thread` alone hosts four roles (attach, workspace creation, auto attach, auto-mode toggling) behind a web of `conflicts_with` rules. Separately, `zerdr thread --session NAME` can only attach to a Herdr session that is already running; there is no way to start a named session from a Zed terminal thread.

## Goal

A restructured CLI with two feature entry points (`connect`, `start`) and two auxiliary groups (`workspace`, `setup`), a single global `--session` definition, and a `connect --create` that also starts a not-running named Herdr session headless before attaching.

## Out of Scope

- Backward compatibility: no aliases for old command spellings, no migration of an installed `terminal_init_command` value, no deprecation period. The tool is pre-release.
- Project-to-session binding (resolving a named session without `--session`).
- Renaming the hidden plugin commands `sync-from-herdr` / `open-from-herdr`.
- Routing/sync support for sessions started by `connect` (they stay sync-less until a `start` wrapper attaches).
- Stopping or deleting Herdr sessions from zerdr.
- Renaming persisted state paths, schemas, or internal module vocabulary.

## Requirements and Decisions

### Requirements

CLI restructure:

- **R1:** The public CLI becomes exactly:
  - `zerdr connect [TARGET] [--session NAME] [--kind KIND] [--create]` (+ hidden `--auto`) — attach a Zed terminal thread to a Herdr pane (old `thread`).
  - `zerdr start [--session NAME] [--anchor PATH]` — launch Herdr wrapped with Zed sync (old bare `zerdr`).
  - `zerdr workspace bind [PATH]` / `zerdr workspace unbind` / `zerdr workspace sync` (old `bind` / `unbind` / `sync`), each accepting `--session`.
  - `zerdr setup install` / `zerdr setup uninstall [--purge]` / `zerdr setup doctor` / `zerdr setup auto <on|off>` (old `setup` / `uninstall` / `doctor` / `thread --enable|--disable`).
  - Hidden, unchanged: `zerdr sync-from-herdr`, `zerdr open-from-herdr`.
- **R2:** Bare `zerdr`, bare `zerdr workspace`, and bare `zerdr setup` print help (usage listing their subcommands) and exit non-zero; they perform no action.
- **R3:** Old spellings are gone: `zerdr thread|sync|bind|unbind|doctor|uninstall` and bare-`zerdr`-as-launcher are rejected as unknown, `zerdr setup` no longer installs, and no user-facing string (error hints, status lines, doctor output, setup output, assets) references an old spelling.
- **R4:** `--session` is defined once as a clap global flag: accepted both before and after the subcommand, at most once. Commands where it is meaningless (`setup install`, `setup uninstall`, `setup auto`, and the hidden plugin commands) reject it with an explicit error, as today.
- **R5:** `--anchor` moves onto `start`; no top-level flags remain. Other commands reject `--anchor` (clap unknown-argument error is sufficient).
- **R6:** `zerdr setup auto on|off` behaves exactly like the old `thread --enable|--disable` (settings backup, fingerprint ownership, flag file), except the installed `agent.terminal_init_command` value becomes `"<executable> connect --auto"`.
- **R7:** `connect --auto` keeps the old `thread --auto` semantics (best-effort, only while auto mode is enabled, silent fallback to a plain shell) and is hidden from help. It still conflicts with `TARGET`, `--kind`, and `--create`.
- **R8:** The generated Zed task launches the wrapper as `[<executable>, "start", "--anchor", "$ZED_WORKTREE_ROOT"]`; setup ownership validation and doctor's task-payload check follow the new payload.
- **R9:** The remote-environment gate (only doctor may run remotely) applies to `setup doctor` in the new tree.

Named-session headless start (`connect --create`):

- **R10:** When `connect --create` targets a named session (explicit `--session NAME`, `NAME != "default"`) that is not running, zerdr starts it headless via `herdr --session NAME server`, waits for its socket to appear, and then proceeds with the normal resolve/create/attach flow. A stopped-but-existing session is started the same way (start and restart are one code path).
- **R11:** Without `--create`, a not-running session remains an error; the message now suggests `--create`. No server process is spawned.
- **R12:** The default session is never auto-started by `connect` (with or without `--create`), and the `--auto` path never starts a session server (a not-running session under auto mode degrades silently to a plain shell, like every other auto failure). The auto-path clause was decided during planning, not in the design session: auto is best-effort and an init command should not silently spawn servers.
- **R13:** Concurrent `connect --create` invocations for the same session name are serialized under a per-session-name lock; exactly one spawns the server, the others observe the socket after it appears.
- **R14:** The server child is spawned detached: separate process group (so Zed's Ctrl-C cannot reach it) with null stdin/stdout/stderr (Herdr writes its own `herdr-server.log`). Readiness polling honors `ZERDR_READY_TIMEOUT_MS` (default 5000 ms, matching the wrapper); on timeout or on the child exiting before the socket appears, zerdr kills/reaps the child where applicable and returns an error naming the session.
- **R15:** When `connect` starts a session, it says so on stdout (one line naming the session) in addition to the existing attachment status line.
- **R16:** Sessions started by `connect` get no route and no wrapper lease; `connect` writes nothing new to the route store on this path. Focus sync stays inert until a `start` wrapper attaches to that session.
- **R17:** zerdr never stops a session it started; no session-stop calls are added anywhere.
- **R18:** Persisted state (bindings, routes, leases, thread memory, install state) keeps its current paths and formats.

### Implementation Decisions

- **D1:** Entry-point names use verbs describing what happens: `connect` (attach this terminal to a Herdr pane — true for attach, reattach, and create alike) and `start` (launch Herdr). Chosen over noun-based (`pane`/`tui`) and over keeping `thread`, which named the problem this rename fixes.
- **D2:** Auxiliary commands nest one level down in two groups: `workspace` (operations on a Herdr workspace: bind, unbind, sync) and `setup` (everything that writes to the user's environment: install, uninstall, doctor, auto). "Bare command = help" is uniform across the tree.
- **D3:** The auto-mode toggle lives under `setup` because enabling it writes to Zed's settings.json — the same class of operation as `setup install`. `connect` becomes a pure "connect" command.
- **D4:** Hidden plugin commands keep their names: they are invisible to users and renaming them only risks breaking installed manifests.
- **D5:** Clean break with no migration machinery, per the user's explicit decision (pre-release tool).
- **D6:** Headless start uses `herdr --session NAME server`, verified working against the real Herdr binary (creates the session directory, socket, and server log without a TUI client).
- **D7:** Internal vocabulary keeps "thread" where it means Zed's terminal threads (`src/thread.rs`, `ThreadLeaseSet`, `thread_auto_flag_file`, state directory names): it is Zed's own domain term and renaming state paths would break persisted state for no user-visible gain. Only the CLI surface and user-facing strings change.
- **D8:** The per-session start lock is keyed by session name (not socket path — the socket does not exist yet), using the existing `OperationGuard` on a lock file derived from the session name. After acquiring the lock, re-check the session list before spawning (double-checked locking) so the loser of a race attaches instead of spawning a second server.

### Contracts

- Installed init command value: `"<executable> connect --auto"` (produced by `setup.rs::terminal_init_command`).
- Generated Zed task args: `["start", "--anchor", "$ZED_WORKTREE_ROOT"]` (template `assets/zed/tasks.json.in`); label stays `"zerdr: Herdr"`.
- Keymap example sends `"zerdr connect\n"`.
- New Herdr adapter capability (name illustrative): `Herdr::spawn_server_detached_for(&self, session_name: &str) -> Result<Child>` — spawns `<program> --session <name> server` in its own process group with null stdio. The caller owns readiness polling and timeout kill; on success the child is intentionally leaked (never waited on after attach begins).
- `thread::run` signature is unchanged (`session_name, target, kind, create`); the not-running-session branch grows inside `run_with_mode`. Whether a session may be started is derivable there: `create && !auto && session_name != DEFAULT_SESSION_NAME`.
- Error message contracts (exact wording chosen at implementation time, content fixed):
  - Not running, no `--create`: names the session and suggests `zerdr connect --create --session <name>`.
  - Not running, default session, `--create`: names the session and points at `zerdr start` (never auto-starts).
  - Start timeout / early child exit: names the session and states the server did not become ready.
- `--session` rejection error names the command that does not take it (content parallel to today's "--session cannot be used with this subcommand").

## Current Context

### Confirmed

- `herdr --session NAME server` starts a named session headless; `herdr session list` (JSON) reports `name`, `running`, `socket_path`. Herdr CLI commands never auto-start a session (they fail with `server_not_running`). Verified live on 2026-08-23.
- `Herdr::find_session_socket` already polls `session list`; `run_wrapper` (`src/herdr.rs:469`) shows the existing readiness-wait pattern with `ZERDR_READY_TIMEOUT_MS` (default 5000, 25 ms interval).
- `sync-from-herdr` no-ops silently when the socket has no live wrapper lease (`src/sync.rs:60-64`), so a session without a wrapper is naturally sync-less — R16 needs no new code, only a doc statement.
- `--session` handling today: duplicated per-subcommand flags plus hand-written once-only/not-accepted checks in `src/lib.rs:38-67`.
- Auto mode: `thread --enable` writes `"<exe> thread --auto"` into `agent.terminal_init_command` with fingerprint ownership (`src/setup.rs:446-494`); `thread::run_auto` gates on the flag file and swallows attach failures.
- Old-spelling hints live in `src/thread.rs` (150, 167), `src/sync.rs` (160, 182, 301, 323), `src/state.rs` (988), `src/doctor.rs` (multiple), `src/setup.rs` (multiple), `assets/zed/keymap.example.json`.
- The fake `herdr` (`tests/support/mod.rs::FAKE_HERDR_BODY`) serves `session list --json` from `$ZERDR_TEST_SESSIONS_JSON` (static) and has no `server` handler yet. AGENTS.md requires the shared `PATH` fake to stay cheap; per-test behavior belongs in `TestEnv::baked_herdr`.
- The remote gate in `src/lib.rs:20-24` matches `Command::Doctor` and must follow doctor into the `setup` group.
- Validation commands and ordering per AGENTS.md: focused test, then `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-targets --all-features`.

### Assumptions

- clap's `arg_required_else_help` (exit code 2, usage output) is an acceptable realization of "bare command prints help" at all three levels; tests assert on content and non-zero exit, not the exact code.
- `setup auto` takes `on`/`off` as a positional value (e.g. an enum arg); `setup auto` with no value shows the group usage.
- While `connect` stays attached, a crashed detached server may linger as a zombie in zerdr's process table until zerdr exits; acceptable (no reaping loop is added for the success path).

## File Structure

- Modify: `src/cli.rs` — new `Command` tree (`Connect`, `Start`, `Workspace(WorkspaceCommand)`, `Setup(SetupCommand)`, hidden pass-throughs); global `--session`; `--anchor` on `Start`.
- Modify: `src/lib.rs` — dispatch over the new tree; single not-accepted `--session` check; remote gate follows `setup doctor`.
- Modify: `src/thread.rs` — not-running-session branch (headless start, lock, readiness wait, status line); updated hint strings.
- Modify: `src/herdr.rs` — `spawn_server_detached_for`; no changes to existing adapters.
- Modify: `src/setup.rs` — `terminal_init_command` value; user-facing strings (`setup install` output, auto-mode messages).
- Modify: `src/doctor.rs` — hint strings (`zerdr setup install`, `zerdr connect`, `zerdr setup auto on`, `zerdr workspace bind`).
- Modify: `src/sync.rs`, `src/state.rs` — hint strings only.
- Modify: `assets/zed/tasks.json.in` — task args gain `"start"`.
- Modify: `assets/zed/keymap.example.json` — SendText `"zerdr connect\n"`.
- Modify: `README.md`, `AGENTS.md` — new command tree, auto-mode section, named-session section with the no-sync statement.
- Test: `tests/cli_contract.rs` — rewritten for the new tree (help content, bare-group help, session rules, conflicts).
- Test: `tests/thread_flow.rs` — invocation renames plus new headless-start cases.
- Test: `tests/sync_flow.rs`, `tests/state_and_bindings.rs`, `tests/setup_and_doctor.rs`, `tests/herdr_wrapper.rs` — invocation and message-assertion renames.
- Test: `tests/support/mod.rs` — fake `herdr` learns `--session NAME server` and file-backed session lists (baked-fake variables, keeping the shared fake cheap).

## Testing Decisions

- **Test seam:** the compiled binary via `assert_cmd` with `TestEnv` fakes, as everywhere else in this repo. No unit-level seams are added.
- **Behavior:** CLI acceptance/rejection matrices in `cli_contract.rs`; end-to-end connect flows (including server spawn ordering read back from the fake's invocation log) in `thread_flow.rs`.
- **Prior art:** `cli_contract.rs` conflict tables; `thread_flow.rs` agent/tab flows; `herdr_wrapper.rs` readiness-wait tests (`ZERDR_TEST_SESSIONS_JSON`, `ZERDR_READY_TIMEOUT_MS`).
- **Avoid:** asserting exact clap help text or exit codes beyond non-zero; depending on real `herdr`/`zed`; putting per-test logic into the shared `PATH` fake; timing-sensitive concurrency tests for R13 (the lock is the same `OperationGuard` mechanism already covered by `state_and_bindings.rs`; R13 is verified by code review plus the double-check-then-spawn structure).

## Progress

- [x] Task 1: CLI restructure (tree, dispatch, strings, assets, all test renames)
- [x] Task 2: Named-session headless start on `connect --create`
- [ ] Task 3: Documentation (README, AGENTS.md) and final sweep

## Tasks

### Task 1: CLI restructure

**Covers:** R1, R2, R3, R4, R5, R6, R7, R8, R9, D1, D2, D3, D4, D5, D7

**Objective:** The binary exposes exactly the new command tree with all user-facing strings and generated assets updated, and the whole existing test suite passes against the new spellings.

**Files:**
- Modify: `src/cli.rs`, `src/lib.rs`, `src/setup.rs`, `src/doctor.rs`, `src/sync.rs`, `src/state.rs`, `src/thread.rs` (strings only in this task)
- Modify: `assets/zed/tasks.json.in`, `assets/zed/keymap.example.json`
- Test: `tests/cli_contract.rs` (rewrite), `tests/thread_flow.rs`, `tests/sync_flow.rs`, `tests/state_and_bindings.rs`, `tests/setup_and_doctor.rs`, `tests/herdr_wrapper.rs` (mechanical renames of invocations and asserted messages)

**Dependencies:** none

**Implementation notes:**
- `Cli.session` becomes `#[arg(long, global = true)]`; delete every per-subcommand `session` field. Keep one hand-written check: a small list of commands that reject `--session` (`setup install|uninstall|auto`, hidden commands), replacing the not-accepted check at `src/lib.rs:62-66`.
- clap does NOT enforce "at most once" for a plain `Option<String>` global arg split across parent and subcommand positions (`--session A connect --session B` parses as last-wins; verified against the pinned clap 4.6.6). Enforce R4's once-only rule explicitly — e.g. declare the global arg as `Vec<String>` (or inspect `ArgMatches` occurrences) and error when more than one value was given, preserving the behavior pinned by `tests/cli_contract.rs::thread_accepts_session_targeting_only_once`.
- `Command::Thread` splits: `Connect { target, kind, create, auto }` keeps the `--kind`/`--create`/`--auto` conflicts against `TARGET` (and `--auto` against the other two) but loses `enable`/`disable`; `SetupCommand::Auto { state: on|off }` takes over the toggle by calling the existing `setup::thread_auto_enable/disable`.
- Bare `zerdr` = help: make the subcommand required with `arg_required_else_help`; same pattern on the `workspace` and `setup` group parsers. The wrapper launch code path moves under `Start { anchor }` (body of today's no-subcommand branch, `src/lib.rs:26-30`, including `runtime::resolve_launch`).
- `--anchor` moves from `Cli` to `Start`; delete the `src/lib.rs:32-36` guard (clap now enforces it).
- The remote gate at `src/lib.rs:20-24` matches the new `Setup(Doctor)` shape.
- `terminal_init_command()` returns `"{exe} connect --auto"`. Auto-mode user messages drop the "thread" spelling (e.g. "auto mode is enabled"); `setup install` output and doctor hints point at `zerdr connect`, `zerdr setup auto on`, `zerdr setup install`, `zerdr workspace bind`.
- Sweep every hint string listed in Current Context. Acceptance check: `rg -n '\bzerdr (thread|bind|unbind|uninstall|doctor)\b|zerdr sync\b|thread --auto|thread --enable|thread --disable' src assets tests README.md AGENTS.md` returns no matches outside `docs/plans/` (README/AGENTS handled in Task 3; if intermediate matches remain there after Task 1, scope the command to `src assets tests`).
- `setup uninstall` keeps `--purge` and all ownership semantics; only the spelling moves.

**Test cases (tests/cli_contract.rs, representative):**
- `zerdr` (bare) → non-zero exit; output lists `connect`, `start`, `workspace`, `setup`; does not list `thread`, `bind`, `doctor` as commands.
- `zerdr thread`, `zerdr sync`, `zerdr bind`, `zerdr doctor`, `zerdr uninstall` → clap unknown-command errors.
- `zerdr workspace` / `zerdr setup` (bare) → non-zero, usage lists their subcommands.
- `--session work connect` and `connect --session work` both parse; `--session` twice → error; `zerdr setup install --session work`, `zerdr setup auto on --session work` → the explicit rejection error.
- `zerdr connect --kind pi wM:p8`, `zerdr connect --create wM:p8`, `zerdr connect --auto --kind pi` → conflict errors; `--auto` absent from `zerdr connect --help` output.
- `zerdr connect --help` → shows `TARGET`, `--kind`, `--create`; no `--enable`/`--disable`.
- `zerdr start --anchor PATH` parses; `zerdr connect --anchor PATH` → unknown argument.
- Setup flow (tests/setup_and_doctor.rs): `setup auto on` writes `"<exe> connect --auto"` into the fake settings file; `setup auto off` removes the flag file; `setup install` produces the Zed task whose args are `["start", "--anchor", "$ZED_WORKTREE_ROOT"]` and doctor accepts it.
- Wrapper flow (tests/herdr_wrapper.rs): `zerdr start` (and `--session work`) drives the existing wrapper scenarios previously driven by bare `zerdr`.

**Complete when:**
- The new tree parses and dispatches per R1–R9; old spellings are rejected.
- The string-sweep `rg` above is clean for `src assets tests`.
- No behavior behind the renamed commands changed (same functions called with the same arguments).

**Validation:**
- Run: `cargo test --test cli_contract && cargo test --test setup_and_doctor && cargo test --test herdr_wrapper`
- Expected: all pass.
- Run: `cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-targets --all-features`
- Expected: all pass.

**Result (2026-08-24):** Implemented and validated; all suites pass (205 tests), fmt/clippy clean, string sweep clean for `src assets tests`. Notes:
- The once-only `--session` rule is enforced by counting raw `--session`/`--session=` argv occurrences after a successful parse (`lib.rs::session_flag_occurrences`), because clap's global-flag propagation makes the subcommand occurrence overwrite the parent one instead of accumulating.
- The rejection message reads "--session cannot be used with this command" (was "…this subcommand").
- The Herdr notification title changed from "zerdr sync failed" to "zerdr: sync failed" so no user-facing string spells an old command.
- `tests/support/mod.rs::prepare_launcher` now runs `setup install`.

### Task 2: Named-session headless start on `connect --create`

**Covers:** R10, R11, R12, R13, R14, R15, R16, R17, R18, D6, D8

**Objective:** `zerdr connect --create --session NAME` against a not-running named session starts it headless, waits for readiness, and proceeds to the existing create/attach flow; all guard rails (no default, no auto, no stop, lock, timeout) hold.

**Files:**
- Modify: `src/herdr.rs` — `spawn_server_detached_for(session_name)` (own process group via `CommandExt::process_group(0)`, null stdio).
- Modify: `src/thread.rs` — replace the flat `session_socket_for` call in `run_with_mode` with: try `session_socket_if_running`; on `None`, either error (R11/R12 wording per Contracts) or, when `create && !auto && session_name != DEFAULT_SESSION_NAME`, run the start sequence; print the R15 status line after readiness.
- Modify: `src/state.rs` — only if a helper is needed to derive the per-session-name lock path; otherwise construct it in `thread.rs` from `Paths` (implementer's choice, following existing lock-path patterns).
- Test: `tests/support/mod.rs`, `tests/thread_flow.rs`.

**Dependencies:** Task 1 (command spelling `connect`).

**Implementation notes:**
- Start sequence under `OperationGuard` on a session-name-keyed lock file: re-check `session_socket_if_running` (double-check, D8); spawn the detached server; poll `find_session_socket` every ~25 ms until the socket appears or `ZERDR_READY_TIMEOUT_MS` (default 5000) elapses, also `try_wait`ing the child so an early exit fails fast; on failure kill/reap the child and return the R14 error; on success release the lock and continue — never wait on or kill the child afterwards (R17).
- The `--auto` path already passes `create=true`; the start gate must therefore check `!auto` explicitly (R12). Under auto, a not-running session falls into `run_auto`'s existing silent-fallback behavior.
- The default-session guard compares against `DEFAULT_SESSION_NAME`; an explicit `--session default` is treated the same as no `--session`.
- Fake `herdr` support: `session list --json` gains an optional file-backed source (e.g. print `$ZERDR_TEST_SESSIONS_FILE` contents when that file exists, else `$ZERDR_TEST_SESSIONS_JSON`); a new `--session <name> server` handler copies `$ZERDR_TEST_SESSIONS_STARTED_JSON` into `$ZERDR_TEST_SESSIONS_FILE` and sleeps. Guard both behind their variables so the shared `PATH` fake stays cheap; drive the new tests through `TestEnv::baked_herdr`.

**Test cases (tests/thread_flow.rs):**
- Not running + `--create --session work` → fake log shows `--session work server` invoked before any `workspace list` for `work`; stdout contains the started-session line and the normal created-workspace status; exit success.
- Not running + `--session work` without `--create` → error mentioning `--create`; fake log contains no `server` invocation.
- Not running default session + `--create` (no `--session`) → error; no `server` invocation.
- Not running + auto mode on + `connect --auto` → exit success as a silent plain shell (existing auto contract); no `server` invocation.
- Server never registers the session (started-JSON equals the original list) + `ZERDR_READY_TIMEOUT_MS=200` → timeout error naming the session; exit non-zero.
- Session already running + `--create --session work` → no `server` invocation; existing attach flow unchanged (guards regression of current behavior).

**Complete when:**
- All six cases pass and the pre-existing `thread_flow` cases still pass.
- No route-store writes occur on the start path (R16) — verified by reading the fake state directory in the first test case or by code review noting no `RouteStore` use is added.

**Validation:**
- Run: `cargo test --test thread_flow`
- Expected: all pass, including the six new cases.
- Run: `cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-targets --all-features`
- Expected: all pass.

**Result (2026-08-24):** Implemented and validated; thread_flow passes 44 tests (6 new), full suite/fmt/clippy clean. Notes:
- The start lock lives at `ThreadLeaseSet::session_start_lock_path` (`session-start-<hash>.lock` under the thread-leases root, hashed by session name).
- `Herdr::spawn_server_detached_for` uses `CommandExt::process_group(0)` with null stdio; on readiness timeout the child is killed and reaped, on success it is intentionally leaked.
- Contract messages as implemented: not-running named session without `--create` → `Herdr session "NAME" is not running; run \`zerdr connect --create --session NAME\` to start it`; default session → `the default Herdr session is not running; launch it with \`zerdr start\``; timeout → `timed out waiting for the NAME Herdr session socket`; start line → `zerdr: started Herdr session NAME`.
- The fake herdr gained a `--session NAME server` handler (writes `$ZERDR_TEST_SESSIONS_STARTED_JSON` to `$ZERDR_TEST_SESSIONS_FILE`, then sleeps `$ZERDR_TEST_SERVER_SLEEP`, default 5s) and a file-backed `session list` source, both env-guarded so the shared fake stays cheap.

### Task 3: Documentation and final sweep

**Covers:** R3 (docs half), R16 (documentation statement), D5

**Objective:** README.md and AGENTS.md describe only the new CLI, including the named-session workflow and its no-sync caveat.

**Files:**
- Modify: `README.md` — command table and every inline example (`zerdr connect`, `zerdr start`, `zerdr workspace …`, `zerdr setup …`); auto-mode section now says `zerdr setup auto on|off` and that the installed value is `zerdr connect --auto`; new short section: `connect --create --session NAME` starts a not-running named session headless, zerdr never stops it, and such sessions have no Zed focus sync until `zerdr start --session NAME` attaches.
- Modify: `AGENTS.md` — repository-map and convention lines that spell old commands (`zerdr thread --enable`, safety list `zerdr setup`/`zerdr uninstall`/`zerdr doctor`).

**Dependencies:** Tasks 1–2 (documented behavior must exist).

**Implementation notes:**
- Keep README user-facing per AGENTS.md; maintainer details stay in AGENTS.md.
- English prose, matching the existing documents.

**Test cases:**
- `rg -n '\bzerdr (thread|bind|unbind|uninstall|doctor)\b|zerdr sync\b|thread --auto|thread --enable|thread --disable' README.md AGENTS.md src assets tests` → no matches (docs/plans excluded).

**Complete when:**
- The sweep is clean and both documents read coherently against `zerdr --help` output.

**Validation:**
- Run: the `rg` sweep above.
- Expected: no matches.
- Run: `cargo run --locked -- --help`
- Expected: help lists `connect`, `start`, `workspace`, `setup` and matches the README table.

## Requirement Coverage

| Requirement / Decision | Task | Verification |
|---|---|---|
| R1 new command tree | Task 1 | cli_contract help/parse matrix |
| R2 bare = help at all levels | Task 1 | cli_contract bare-invocation cases |
| R3 old spellings removed | Task 1, 3 | unknown-command cases + `rg` sweep |
| R4 global `--session` + explicit rejection | Task 1 | cli_contract session matrix |
| R5 `--anchor` only on `start` | Task 1 | cli_contract anchor cases |
| R6 `setup auto` writes `connect --auto` | Task 1 | setup_and_doctor settings assertion |
| R7 hidden `--auto`, semantics unchanged | Task 1, 2 | cli_contract help assertion + thread_flow auto case |
| R8 Zed task uses `start --anchor` | Task 1 | setup_and_doctor task payload assertion |
| R9 remote gate follows `setup doctor` | Task 1 | existing remote doctor test updated in cli_contract/setup_and_doctor |
| R10 headless start + proceed | Task 2 | thread_flow case 1 |
| R11 no `--create` → error, no spawn | Task 2 | thread_flow case 2 |
| R12 default/auto never start servers | Task 2 | thread_flow cases 3–4 |
| R13 per-name lock, single spawn | Task 2 | code review per Testing Decisions (OperationGuard reuse, double-check) |
| R14 detached spawn + timeout handling | Task 2 | thread_flow case 5; process-group/null-stdio by code review |
| R15 started-session status line | Task 2 | thread_flow case 1 stdout assertion |
| R16 no route/lease for started sessions | Task 2, 3 | thread_flow case 1 state check + README statement |
| R17 no session-stop calls | Task 2 | code review: no stop invocation added; fake log has no `stop` |
| R18 persisted state unchanged | Task 1, 2 | state_and_bindings suite passes unmodified except command spellings |
| D1–D5, D7 | Task 1 | structure realized in cli.rs/lib.rs per notes |
| D6, D8 | Task 2 | implementation notes; thread_flow cases 1, 5 |

## Final Validation

- [ ] `cargo test --test cli_contract` — Expected: pass
- [ ] `cargo test --test thread_flow` — Expected: pass (six new cases included)
- [ ] `cargo fmt --all -- --check` — Expected: no diff
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` — Expected: no warnings
- [ ] `cargo test --all-targets --all-features` — Expected: all pass
- [ ] `rg -n '\bzerdr (thread|bind|unbind|uninstall|doctor)\b|zerdr sync\b|thread --auto|thread --enable|thread --disable' README.md AGENTS.md src assets tests` — Expected: no matches
- [ ] Manual: from a real Zed terminal thread in this checkout, `zerdr connect --create --session scratch` starts and attaches; `herdr session stop scratch` afterwards (developer machine; not part of automated tests per AGENTS.md safety rules)
- [ ] Requirement Coverage has no unaddressed rows
- [ ] The plan matches the actual changes
- [ ] After all of the above succeed, move this file unchanged to `docs/plans/archived/`

## Risks and Open Questions

- Risk: `arg_required_else_help` on nested groups can interact oddly with global flags (`zerdr --session x` alone). Tests must pin that this shows help/error rather than launching anything.
- Risk: the detached server's readiness window races Herdr's own session-directory creation; the poll loop must tolerate `session list` briefly not listing the new session (it already does — absence is `Ok(None)`).
- Risk: `setup uninstall` in an environment that recorded the old fingerprint value will not recognize the new expected string; per D5 (no migration) this is accepted — uninstall still removes the value matching the *recorded* fingerprint, which is the value it wrote.
- Open questions: none.
