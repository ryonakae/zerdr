# Automatic Internal and External Terminal Routing Implementation Plan

> **For implementers:** Execute tasks in order unless dependencies allow otherwise. Mark a task complete only after its validation succeeds. Reflect minor implementation differences in the relevant task. Ask the user before changing requirements, Out of Scope, or public contracts.

## Problem Statement

zerdr currently solves Herdr-to-Zed synchronization only when Herdr is launched through the generated `zerdr: Herdr` task inside a project-backed Zed terminal. That launch path is difficult to discover, and it addresses only one side of the original workflow problem. A user running Herdr in Ghostty, iTerm, or another local terminal still wants a focused Herdr workspace to open or activate the corresponding Zed project.

The desired primary interface is now bare `zerdr`. It must detect whether it runs in a Zed integrated terminal, choose the existing single-managed-window routing there, and otherwise choose an external routing strategy that asks Zed to activate the selected project wherever Zed can place it. The external flow should leave the terminal application foreground on macOS when possible, without requiring Accessibility permission or adopting private Zed/macOS APIs.

## Goal

Deliver one automatic launcher with two explicit, lease-authorized routing modes:

- **internal:** launch Herdr from a local Zed terminal, use the current Git checkout as the bootstrap anchor, and preserve the existing anchored `--existing` → `--add` pipeline;
- **external:** launch Herdr from a supported local non-Zed terminal, perform no startup Zed sync, and run `zed --existing TARGET` for each later focused workspace.

The implementation must preserve the fixed `zerdr` Herdr session, one-live-wrapper invariant, canonical Git-root identity, binding behavior, setup ownership rules, failure notifications, and distribution gates.

## Out of Scope

- Guaranteeing “reuse an existing matching window, otherwise always create a new window.” Zed's public CLI does not expose that hybrid contract.
- Enumerating Zed windows/projects or selecting a Zed window by ID.
- Using Zed private state, a Zed extension, macOS private frameworks, or undocumented Zed IPC.
- Requiring Accessibility or Automation permission, or using `AXRaise` to map project roots to OS windows.
- Guaranteeing that Ghostty/iTerm never loses focus even transiently; macOS restoration is best-effort after Zed handles the request.
- Focus restoration on Linux, X11, or Wayland.
- SSH, WSL, dev-container, container, remote, or non-local checkout support.
- More than one live wrapper, including multiple wrappers in the same routing mode.
- Automatically replacing a live wrapper when a launcher requests another mode.
- An always-on plugin daemon or synchronization from ordinary Herdr clients when no zerdr wrapper lease is live.
- Installing, detecting, warning about, or interoperating with `artisann.zed-herdr`.
- Ensuring every Herdr workspace in Zed at startup; only the focused workspace is handled on demand.
- Automatic setup during launch.
- Publishing a release, creating a tag, or mutating the Homebrew tap without separate approval.

## Requirements and Decisions

### Requirements

- **R1 — Bare launcher:** Running `zerdr` with no subcommand launches or attaches the fixed `zerdr` Herdr session. The public `zerdr herdr` subcommand is removed.
- **R2 — Automatic mode:** `auto` resolves to `internal` only when `ZED_TERM=true` and `TERM_PROGRAM=zed`; all other supported local terminals resolve to `external`.
- **R3 — Explicit mode and anchor:** `--mode auto|internal|external` overrides detection. `--anchor PATH` implies `internal`, may be used outside a detected Zed terminal, and conflicts with explicit `external`. `internal` uses the explicit anchor or the canonical Git root containing CWD and fails before child spawn when no valid root exists.
- **R4 — Internal precondition:** An automatically derived or explicit internal anchor is assumed to belong to the intended Zed window. zerdr documents that it cannot verify this through the public Zed API.
- **R5 — External focus policy:** `--focus terminal|zed` applies only to external routing. On macOS, omitted focus defaults to `terminal`; on Linux it defaults to `zed`, and explicit `terminal` is rejected as unsupported before child spawn.
- **R6 — External sync:** External focus handling resolves only the focused canonical Git root and invokes exactly `zed --existing TARGET`. It performs no `--add`, anchor promotion, or all-workspace ensure pass.
- **R7 — Startup behavior:** Internal mode retains startup sync. External mode establishes route authority and shows Herdr without invoking Zed until a later `workspace.focused` event or explicit manual sync.
- **R8 — Event semantics:** Eligible focus events are not deduplicated. Each event rereads current Herdr focus under the socket lock and performs the mode-specific route. If Herdr emits no event for an already-focused workspace, `zerdr sync` is the explicit retry/refocus operation.
- **R9 — Best-effort macOS restoration:** With external `--focus terminal`, zerdr records the frontmost application immediately before invoking Zed. After Zed returns, it restores that application only if the application observed by the post-command check is Zed. If that check observes another non-Zed application, no restoration occurs. A user switch in the unavoidable interval after the check and before activation can still be overridden. Restoration failure is silent and never changes Zed sync success/failure.
- **R10 — Platform boundary:** External synchronization remains available on macOS and Linux. Only macOS implements terminal focus restoration. Remote detection is authoritative and cannot be overridden: SSH is detected by nonempty `SSH_CONNECTION`, `SSH_CLIENT`, or `SSH_TTY`; WSL by nonempty `WSL_DISTRO_NAME` or `WSL_INTEROP`; container/dev-container by nonempty `container`, `REMOTE_CONTAINERS`, `DEVCONTAINER`, or `CODESPACES`, or by `/.dockerenv` or `/run/.containerenv`. Detection collects every matching marker and reports them in this fixed order: `SSH_CONNECTION`, `SSH_CLIENT`, `SSH_TTY`, `WSL_DISTRO_NAME`, `WSL_INTEROP`, `container`, `REMOTE_CONTAINERS`, `DEVCONTAINER`, `CODESPACES`, `/.dockerenv`, `/run/.containerenv`. Detected remote environments reject all commands that launch, mutate, or synchronize. `--help` and `--version` remain available. `doctor` performs static read-only installation inspection, reports all detected markers in that order, and skips Herdr/Zed process calls, locks, lease/route cleanup, and every other state mutation.
- **R11 — One authority:** Internal and external wrappers share the fixed session and one locked lease. Any second wrapper is rejected without replacing route state, regardless of requested mode. Mode switching requires closing the live wrapper first.
- **R12 — Manual commands:** `pick`, `next`, `previous`, `sync`, `bind`, and `unbind` no longer require a Zed terminal. They require one valid live lease and obey the owning route's internal/external behavior.
- **R13 — Failure behavior:** Root resolution, Zed command, route, and notification failures retain existing no-rollback semantics. Zed or focused-root failures never terminate an admitted Herdr UI or release its lease. Focus restoration is excluded from error delivery.
- **R14 — Setup preflight and task migration:** Before child spawn, a launcher requires all of the following: plugin-list entry `plugin_id="zerdr"`, `enabled=true`, and an event with `on="workspace.focused"`; readable install state with a supported install schema; and a materialized manifest with `id="zerdr"`, `min_herdr_version="0.8.0"`, exactly one `workspace.focused` event, and command `[CURRENT_EXECUTABLE, "sync-from-herdr"]`. CURRENT_EXECUTABLE comparison resolves both paths to the same executable identity so symlink/relative spelling does not cause a mismatch. Failure gives `zerdr setup` guidance. Zed task presence or modification does not block bare launch. Setup retains five owned task labels and migrates `zerdr: Herdr` to `zerdr --mode internal --anchor "$ZED_WORKTREE_ROOT"` while preserving ownership/JSONC guarantees.
- **R15 — Diagnostics:** `doctor` reports the live route mode, validates an anchor only for internal routes, reports external focus policy/platform support, uses bare-`zerdr` recovery guidance, and continues per-socket stale cleanup and exact lease/PID checks.
- **R16 — Compatibility and quality:** Existing binding, lease, picker, notification, setup rollback, task ownership, package, CI, and release workflow contracts remain covered. Existing route files from the current internal-only schema remain readable during an in-place binary upgrade.

### Implementation Decisions

- **D1 — Wrapper-scoped authority:** Keep the current child-owned wrapper and locked lease instead of adopting an always-on plugin daemon. Routing occurs only while bare zerdr owns the session.
- **D2 — Persist resolved strategy:** Resolve `auto`, platform defaults, anchor, and focus policy before child spawn; persist only a concrete internal or external strategy in socket-scoped route state.
- **D3 — Direct external routing:** Reuse the existing `Zed::activate_existing` process boundary. The accepted fallback is that an unopened project may be added to an eligible existing multi-project window instead of receiving a new window.
- **D4 — No external startup focus:** External launch intentionally does not call Zed because doing so would immediately take focus away from the newly opened Herdr terminal.
- **D5 — AppKit, not Accessibility:** macOS focus restoration uses public frontmost/running-application APIs through a target-specific Rust boundary. It does not identify individual Zed windows; Zed CLI selects and orders the target window first.
- **D6 — Best-effort race guard:** Restore the captured application only when the post-command frontmost application is recognized as Zed. This avoids pulling the user back when a non-Zed app is already visible at the check, but it cannot make the check and activation atomic; a later user switch may still be overridden.
- **D7 — Separate route schema version:** Keep binding and lease schemas at their current version. Introduce a route-specific schema version so evolving routing metadata does not invalidate unrelated persisted state.
- **D8 — Optional task fallback:** Bare `zerdr` is the documented primary launch. The generated long-running Zed task remains a precise-anchor fallback and migration target, not a prerequisite for bare launch.
- **D9 — No third-party coordination:** Do not inspect or special-case zed-herdr installation. Conflicting synchronization plugins are a user configuration error.

### Contracts

#### Public CLI

```text
zerdr [--mode <auto|internal|external>] [--anchor <PATH>] [--focus <terminal|zed>]
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

Resolution rules:

| Input/environment | Result |
|---|---|
| `zerdr` in Zed terminal | internal with canonical CWD Git root |
| `zerdr` in supported non-Zed local terminal | external with platform-default focus policy |
| `zerdr --anchor PATH` | internal with canonical PATH root |
| `zerdr --mode internal [--anchor PATH]` | internal with PATH or canonical CWD root |
| `zerdr --mode external` | external |
| `--mode external --anchor PATH` | usage failure before child spawn |
| internal plus `--focus ...` | usage failure before child spawn |
| Linux external plus `--focus terminal` | unsupported failure before child spawn |
| any mutating/runtime command in detected SSH/WSL/container | local-only failure before process call, lock, or state mutation; explicit mode cannot override |
| remote `zerdr doctor` | static install report plus remote-boundary warning; no Herdr/Zed call or state cleanup |
| any launch-only option plus any subcommand, in either argument order | usage failure before dispatch |
| `zerdr herdr ...` | unknown subcommand/usage failure |

`--mode`, `--anchor`, and `--focus` are launch-only options. Combining any of them with any public or hidden subcommand is a usage failure, regardless of whether the option appears before or after the subcommand. `--help` and `--version` retain their standard Clap behavior. In a detected remote environment, `doctor` remains runnable only in its static read-only form: it reports which authoritative marker caused rejection and performs no external process call, lock acquisition, stale cleanup, or write.

#### Owned Zed fallback task

Setup owns this decision-bearing payload; `command` preserves the current `shell_quote(executable)` representation (including quoting paths with spaces) and the full normalized object receives the new ownership fingerprint.

```json
{
  "label": "zerdr: Herdr",
  "command": "<existing shell_quote(setup-resolved-zerdr-executable) representation>",
  "args": ["--mode", "internal", "--anchor", "$ZED_WORKTREE_ROOT"],
  "allow_concurrent_runs": true,
  "use_new_terminal": true,
  "reveal": "always",
  "hide": "never"
}
```

#### Route state

New writes use a route-specific versioned tagged strategy. Binding and lease schema versions do not change.

```json
{
  "schema_version": 2,
  "session_name": "zerdr",
  "socket_path": "/canonical/herdr.sock",
  "wrapper_pid": 1234,
  "routing": {
    "mode": "internal",
    "anchor_root": "/canonical/git/root"
  }
}
```

or:

```json
{
  "schema_version": 2,
  "session_name": "zerdr",
  "socket_path": "/canonical/herdr.sock",
  "wrapper_pid": 1234,
  "routing": {
    "mode": "external",
    "focus": "terminal"
  }
}
```

Invariants:

- route socket/session validation and exact route PID = sole locked lease PID remain mandatory before Herdr/Zed calls;
- internal anchors are canonical local Git roots and promote only after `--existing CURRENT_ANCHOR` and `--add TARGET` succeed;
- external routes have no anchor and are never promoted;
- external focus policy is already resolved for the host platform before persistence;
- current schema-v1 route records deserialize as internal routes with their existing `anchor_root`; new writers never emit v1;
- an authenticated successful internal event/manual sync through a live v1 route performs the existing two-command pipeline and rewrites v2 only at successful promotion; any preflight/Zed/write failure leaves the original v1 bytes unchanged;
- unsupported/malformed route bytes are not overwritten by event handling.

#### Focus restoration

The platform boundary observes this behavior:

```text
capture frontmost app
  -> invoke zed --existing TARGET
  -> inspect frontmost app
  -> if it is Zed, request activation of captured app
  -> otherwise no-op
```

It runs for external `focus=terminal` only. Supported Zed bundle identifiers are exactly `dev.zed.Zed`, `dev.zed.Zed-Preview`, `dev.zed.Zed-Nightly`, and `dev.zed.Zed-Dev`, matching Zed's public release-channel IDs. An unknown/custom bundle identity disables restoration silently for that call. Restoration executes on both Zed success and Zed failure paths, is never reported as a sync error, and does not require Accessibility permission. The post-command check and activation are not atomic. Different macOS Spaces may move transiently. Linux has no restoration implementation.

#### Lifecycle

```text
resolve local environment + concrete mode + anchor/focus
  -> verify compatible enabled Herdr plugin
  -> spawn/attach fixed Herdr session
  -> discover socket
  -> under lifecycle + socket locks:
       reject any live wrapper
       write concrete route
       acquire one lease
  -> internal: nonfatal startup sync
     external: no startup Zed call
  -> wait for Herdr child; route may remain stale after lease removal
```

## Current Context

### Confirmed

- Current repository HEAD is the initial MVP at commit `4444227` with 67 passing tests before this change.
- Zed-terminal detection already uses exact `ZED_TERM=true` and `TERM_PROGRAM=zed` checks in `src/runtime.rs`.
- The wrapper already canonicalizes an anchor before spawning, owns one fixed-session lease, serializes admission, and retains the UI after internal startup-sync failure.
- Plugin focus events already authenticate by canonical socket plus locked lease and serialize through the socket sync lock.
- Real Zed 1.15.0 dogfooding confirmed that direct `zed --existing TARGET` focuses an existing matching project and can open a different window when the target is absent. Current Zed source also permits an unmatched `--existing` directory to be added to an eligible existing multi-project window, so new-window creation is not a contract.
- The current generated task safely supports `allow_concurrent_runs: true` plus `use_new_terminal: true`, allowing zerdr rather than Zed to reject a competing launcher.
- `zed-herdr` HEAD `8835ffd` independently demonstrates external Herdr event handling through `zed -e ROOT`, but it does not guarantee unopened-project new-window routing and is not an interoperability target.
- Apple documents `kAXRaiseAction`, but project-to-Zed-window mapping is not reliable for multi-project windows. The agreed design therefore restores the prior application after Zed CLI selection and does not use AX APIs.
- Zed's release-channel source defines the stable application IDs `dev.zed.Zed`, `dev.zed.Zed-Preview`, `dev.zed.Zed-Nightly`, and `dev.zed.Zed-Dev`; local Zed 1.15.0 reports `dev.zed.Zed`.

### Assumptions

- The concrete Rust AppKit binding and module-private type names may follow the compatible crate API selected during implementation; they do not alter the public focus contract above.
- Test-only environment seams may be extended to fake platform, frontmost application identity, and restoration outcomes without becoming public configuration.

## File Structure

- Modify: `Cargo.toml` — add macOS-target-only public AppKit/Foundation bindings selected for frontmost-app observation and activation.
- Modify: `Cargo.lock` — lock the target-specific dependency update.
- Modify: `src/cli.rs` — optional subcommand, bare launch options, public mode/focus enums, and removal of `herdr`.
- Modify: `src/lib.rs` — dispatch bare launch, apply local-environment policy, and remove blanket Zed-terminal guards from manual commands.
- Modify: `src/runtime.rs` — resolve terminal mode, remote/container rejection, CWD/anchor rules, platform focus defaults, and launch-only option validation.
- Create: `src/focus.rs` — platform boundary for capture/conditional restore with macOS implementation and non-macOS contract.
- Modify: `src/state.rs` — route-specific schema version, tagged internal/external route strategy, v1 read compatibility, validation, and internal-only promotion.
- Modify: `src/herdr.rs` — accept a resolved route strategy, plugin preflight, mode-neutral admission, internal-only startup sync, and mode-aware conflict errors.
- Modify: `src/sync.rs` — branch internal/external routing after authority/root validation and permit manual commands from any supported local terminal.
- Modify: `src/zed.rs` — surround external `--existing` calls with the focus-restoration boundary while preserving process error semantics.
- Modify: `src/setup.rs` — migrate the owned Herdr task payload and expose/reuse compatible-plugin validation for launcher preflight.
- Modify: `src/doctor.rs` — route-mode diagnostics, platform focus capability, remote boundary reporting, and bare-launch recovery text.
- Modify: `src/error.rs` — mode-neutral launcher/manual recovery messages and actionable option/platform errors.
- Modify: `assets/zed/tasks.json.in` — optional fallback task command without the removed subcommand.
- Modify: `README.md` — bare launch quickstart, mode behavior, external limitations, focus policy, remote exclusions, and optional task fallback.
- Create: `docs/plans/2026-08-19-auto-terminal-routing.md` — this contract, implementation progress, and E2E record.
- Modify: `docs/plans/2026-08-18-zerdr-mvp.md` — point conflicting launcher/external-terminal claims to this revision without rewriting historical tasks.
- Modify: `tests/cli_contract.rs` — bare CLI, mode/option matrix, removed subcommand, and remote policy.
- Modify: `tests/state_and_bindings.rs` — route v2 internal/external persistence, validation, promotion, and v1 compatibility.
- Modify: `tests/herdr_wrapper.rs` — mode-specific admission/startup/plugin-preflight behavior and conflicts.
- Modify: `tests/sync_flow.rs` — exact external command behavior, repeat events, manual commands, and focus-restoration orchestration.
- Modify: `tests/setup_and_doctor.rs` — task migration, optional-task launcher boundary, plugin preflight, and mode-aware doctor output.
- Modify: `tests/support/mod.rs` — deterministic fake Git/Herdr/Zed process logging plus remote/platform, frontmost-app, and restoration seams.

## Testing Decisions

- **CLI seam:** invoke the compiled binary through `assert_cmd` with explicit environment matrices and extend the fake process seam to record Git as well as Herdr/Zed. Option-conflict, platform-policy, and remote rejections occur before Git/Herdr/Zed and have an empty process log. Internal root resolution may run only the expected `git rev-parse` before succeeding or reporting a non-Git root. Plugin-preflight failures may include that prior internal Git query plus the expected read-only `herdr plugin list`; assert no `herdr --session zerdr` UI child, Zed call, route write, or lease.
- **State seam:** inspect versioned route JSON and behavior through `RouteStore`; do not assert private serialization helpers beyond the documented JSON contract.
- **Process seam:** retain fake Herdr/Zed executables and exact argument logs. External success must contain one `zed --existing TARGET` and no `--add`; internal regressions retain exact two-command order.
- **Focus seam:** inject a deterministic platform focus backend. Assert capture/restore order, the non-atomic Zed-observation race guard, all four supported Zed bundle IDs, unknown-ID no-op, Linux rejection, and silent restoration failure without invoking real AppKit in automated tests.
- **Concurrency seam:** retain real file locks and child processes for one-winner admission, route PID authority, queued-event lease revalidation, doctor cleanup, and purge races.
- **Setup seam:** continue byte/fingerprint ownership checks against JSONC and exact generated task payloads.
- **Manual E2E:** use real Zed 1.15.0+, Herdr 0.8.0+, Ghostty on macOS, and disposable Git roots. Automated tests cannot prove OS-window ordering, Space behavior, or final frontmost application.
- **Avoid:** window-title matching, AX permissions, private Zed storage/IPC, sleeps as correctness synchronization, tests coupled to module-private enum layout, and claims that an absent external target always gets a new window.

## Progress

- [x] Task 1: Establish the bare CLI, automatic mode resolution, local-only boundary, and versioned route contracts.
- [x] Task 2: Make wrapper admission and setup migration mode-aware while preserving one authority.
- [x] Task 3: Deliver external on-demand sync and terminal-independent manual commands.
- [x] Task 4: Add best-effort macOS terminal-focus restoration behind a platform boundary.
- [x] Task 5: Complete mode-aware diagnostics and public documentation.
- [ ] Task 6: Complete automated regression and real internal/external E2E validation.

### Validation record (2026-08-19)

- Commits `dba4158`, `0a80943`, and `635bf30` implement Tasks 1-5 plus the independent-review authority/preflight fixes.
- Local macOS gates at `635bf30`: format, Clippy with warnings denied, all 85 unit/integration tests, metadata, package listing, `actionlint`, and `git diff --check` passed.
- Hosted run [32167081756](https://github.com/ryonakae/zerdr/actions/runs/32167081756) passed on `macos-latest` and `ubuntu-latest` for `0a80943`. A post-fix hosted run is required before archival.
- Independent review found no blocking issue after the picker authority race fix and reported **Ready to merge: Yes**.
- Real Zed/Herdr/Ghostty E2E has not run for this revision. Task 6 and plan archival remain incomplete.
- No tag, GitHub Release, or Homebrew tap mutation occurred.

Implementation-time minor file changes or internal differences must be reflected in the relevant task. Ask the user before changing requirements, Out of Scope, public contracts, persisted schemas, or task labels.

## Tasks

### Task 1: Establish Bare Launch and Route Contracts

**Covers:** R1-R5, R10, R12, R16, D2, D7

**Objective:** Parse bare launch safely, resolve one concrete mode before side effects, reject unsupported environments/options, and persist a backward-readable route strategy.

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/lib.rs`
- Modify: `src/runtime.rs`
- Modify: `src/state.rs`
- Modify: `src/error.rs`
- Modify: `src/herdr.rs` — preserve current internal launch through the new route API until Task 2 adds external lifecycle behavior.
- Modify: `src/sync.rs` — preserve current internal sync through the tagged/v1-compatible route API until Task 3 adds external routing.
- Modify: `src/doctor.rs` — implement remote static read-only dispatch and preserve local internal route diagnosis through the new route API; Task 5 adds final external/focus presentation.
- Modify: `tests/cli_contract.rs`
- Modify: `tests/state_and_bindings.rs`
- Modify: `tests/setup_and_doctor.rs` — remote doctor non-mutation and internal route compatibility.
- Modify: `tests/herdr_wrapper.rs` — adapt direct route assertions to the tagged strategy while retaining current internal expectations; Task 2 adds mode lifecycle cases.
- Modify: `tests/sync_flow.rs` — adapt direct route assertions to the tagged/v1-compatible API while retaining current internal expectations; Task 3 adds external routing.

**Dependencies:** Current MVP route/lease and canonical-root behavior.

**Implementation notes:**
- Start with failing public CLI and route behavior tests.
- Model subcommands as optional so no subcommand means launch; do not add a hidden compatibility alias for `herdr`.
- Treat all launch options as mutually exclusive with the presence of any subcommand in either argument order; do not silently ignore options on administrative, manual, doctor, or hidden commands.
- Resolve launch options into a concrete internal/external route before plugin checks, child spawn, route writes, or lease acquisition.
- Detect remote environments using the exact R10 marker set before terminal auto-detection; a marker wins over Zed env, explicit mode, anchor, or focus. In remote doctor mode, avoid even lifecycle-lock creation/acquisition and stale cleanup.
- Split route schema versioning from existing binding/lease constants. Read v1 route bytes as internal without rewriting them during load. Update current wrapper, synchronizer, and doctor call sites in the same task with internal-only compatibility adapters so the crate and existing regression tests remain buildable before Tasks 2-3 add new behavior.
- Preserve invalid-route bytes on load/event failure.

**Test cases:**
- bare command + exact Zed env + Git CWD → resolved internal route with canonical CWD anchor.
- bare command + no Zed env on macOS seam → resolved external/terminal route.
- bare command + no Zed env on Linux seam → resolved external/zed route.
- explicit internal outside Zed + Git CWD → allowed.
- explicit external inside Zed → external.
- anchor in auto mode → internal and canonicalized.
- external + anchor, internal + focus, or Linux terminal focus → actionable pre-spawn failure and empty Git/Herdr/Zed log.
- invalid/non-Git internal anchor or CWD → only the expected Git resolution is logged; no Herdr/Zed/route/lease side effect.
- `zerdr herdr` → unknown-subcommand usage failure.
- each exact SSH/WSL/container env/file marker + each mutating/runtime command → local-only failure with all marker sources in fixed R10 order; explicit internal/external cannot override.
- multiple simultaneous remote markers in scrambled environment order → one deterministic fixed-order report.
- remote doctor → reports all markers in fixed order, reads only static install files, and produces no Herdr/Zed log, lock file, cleanup, or changed bytes.
- each launch option (`mode`, `anchor`, `focus`) + every public/hidden subcommand, with option before and after subcommand → usage failure and no dispatch.
- v2 internal and external routes → exact documented shape and validation.
- v1 route fixture → loads as internal and retains anchor/PID authority.
- external promotion attempt, malformed mode/focus, noncanonical internal anchor, socket mismatch → failure without overwrite.

**Complete when:**
- CLI and route contracts are fixed by behavior tests.
- Option/platform/remote rejections start no process and mutate no state. Root rejection may run Git only. Plugin-preflight rejection may run prior internal Git resolution and its documented read-only plugin query, but never starts the Herdr UI or Zed and never writes route/lease state.
- Existing binding and lease schema tests remain unchanged and passing.

**Validation:**
- Run: `cargo test --test cli_contract && cargo test --test state_and_bindings && cargo test --test setup_and_doctor && cargo test --all-targets --no-run`
- Expected: bare/mode/remote matrices, remote-doctor non-mutation, both route schema generations, existing internal setup/doctor regressions, and every unit/integration test target compile against the tagged route API.

### Task 2: Make Wrapper Authority and Setup Mode-Aware

**Covers:** R7, R11, R13, R14, R16, D1, D2, D4, D8

**Objective:** Launch either mode through the existing fixed-session lifecycle, require the zerdr plugin, preserve one authority, and migrate the optional Zed task safely.

**Files:**
- Modify: `src/herdr.rs`
- Modify: `src/lib.rs`
- Modify: `src/setup.rs`
- Modify: `src/error.rs`
- Modify: `assets/zed/tasks.json.in`
- Modify: `tests/herdr_wrapper.rs`
- Modify: `tests/setup_and_doctor.rs`
- Modify: `tests/support/mod.rs`

**Dependencies:** Task 1 concrete route type and bare launch dispatch.

**Implementation notes:**
- Reuse the current `ManagedChild`, readiness timeout, signal forwarding, lifecycle lock, socket lock, route write, and lease acquisition sequence.
- Verify the exact R14 plugin-list, install-state, manifest, and current-executable predicate before child spawn. Reuse parsing/inspection logic with doctor rather than maintaining two definitions. Do not inspect generated Zed tasks or task fingerprints for launch eligibility.
- Persist the resolved route before acquiring the lease, under the same admission locks as today.
- Keep internal startup sync nonfatal. Skip all startup workspace/Zed calls for external routes.
- Report both requested and live route mode when rejecting a candidate where possible; never terminate the owner.
- Update only fingerprint-owned task payloads. The migrated Herdr task has exact args `["--mode", "internal", "--anchor", "$ZED_WORKTREE_ROOT"]` and retains `allow_concurrent_runs: true`, `use_new_terminal: true`, `reveal: "always"`, and `hide: "never"`; setup records the new full-payload fingerprint. Preserve foreign/modified labels, JSONC, rollback, idempotence, and no-keymap behavior.

**Test cases:**
- missing/disabled/plugin-list-event mismatch, missing/unsupported install state, malformed/wrong manifest, manifest executable resolving to a different binary, or event command mismatch → external launch logs only the expected plugin-list query; internal launch may first log canonical Git resolution; neither starts a UI child/Zed call nor writes route/lease state; setup guidance returned.
- current executable reached through a symlink or relative spelling but resolving to the manifest executable → compatible.
- compatible plugin/manifest + missing/modified optional Zed task → bare launch remains allowed.
- internal launch → route/lease established, startup two-command sync retained.
- external launch → external route/lease established, Herdr remains live, no startup workspace or Zed log.
- external and internal candidates racing from no owner → one winner; loser child reaped; route mode/PID belong to winner.
- same-mode or opposite-mode second launch → candidate failure while owner UI, lease, and route remain.
- internal/external child exit and signal paths → lease cleanup/session preservation unchanged.
- setup from current five-task install → updates only Herdr task to the exact payload above, refreshes fingerprint, and remains byte-stable on repetition.
- uninstall/rollback/modified task cases → existing ownership behavior retained.

**Complete when:**
- Both launch modes use one lifecycle without weakening race safety.
- External launch demonstrably makes no startup Zed call.
- Setup migration is idempotent and optional-task absence does not block launcher preflight.

**Validation:**
- Run: `cargo test --test herdr_wrapper && cargo test --test setup_and_doctor`
- Expected: mode-specific lifecycle, plugin preflight, one-owner races, and task migration pass.

### Task 3: Deliver External On-Demand Sync and Universal Manual Commands

**Covers:** R6-R8, R12-R13, R16, D3, D4

**Objective:** Branch the authenticated sync pipeline by persisted route strategy and allow manual commands from any supported local terminal.

**Files:**
- Modify: `src/sync.rs`
- Modify: `src/zed.rs`
- Modify: `src/lib.rs`
- Modify: `src/error.rs`
- Modify: `tests/sync_flow.rs`
- Modify: `tests/cli_contract.rs`
- Modify: `tests/support/mod.rs`

**Dependencies:** Tasks 1-2 route/lease authority and wrapper modes.

**Implementation notes:**
- Keep socket lock acquisition, exact one-live-PID check, route/PID match, focused workspace reread, and canonical root resolution common to both modes.
- Internal branch remains `--existing ANCHOR` → `--add TARGET` → successful promotion.
- External branch is one `--existing TARGET` and no route mutation.
- Do not cache the last workspace/root. Each eligible event executes the external command.
- Remove terminal-origin checks from manual commands; rely on session socket, live lease, and route authority.
- Keep navigation's single-owner rule: focus-changing manual commands ask Herdr to focus and plugin event owns Zed routing; current selection/direct sync uses the common pipeline directly.
- Replace Zed-specific recovery text with bare `zerdr` and current live-mode guidance.

**Test cases:**
- external focused event → exactly one `zed --existing TARGET`, no `--add`, route bytes unchanged.
- authenticated event and manual sync with one matching live lease plus a v1 internal route → exact existing/add pipeline; successful promotion rewrites v2 with the same owner and target anchor.
- existing/add/root failure through that live v1 route → original v1 bytes remain unchanged.
- same external event twice → two exact Zed calls.
- absent/malformed/mismatched route or wrong/multiple lease PIDs → notification and no workspace/Zed call.
- queued external event whose lease expires → no Zed call.
- external Zed failure or invalid root → one notification, route/lease/UI retained.
- internal success/failure/promotion tests → unchanged command order and rollback.
- pick/next/previous/sync/bind/unbind from non-Zed local terminal with internal owner → obey internal route.
- same commands with external owner → obey external route.
- manual command without live lease → mode-neutral actionable error instructing `zerdr`.

**Complete when:**
- Every sync entry point selects behavior only from the authenticated persisted route.
- External mode is on-demand and non-deduplicating.
- Existing internal, binding, picker, task-delivery, and notification tests remain passing.

**Validation:**
- Run: `cargo test --test sync_flow && cargo test --test cli_contract`
- Expected: exact external/internal process logs, universal manual commands, authority failures, and existing regressions pass.

### Task 4: Add Best-Effort macOS Terminal Focus Restoration

**Covers:** R5, R9-R10, R13, D5-D6

**Objective:** Leave the event-start frontmost application active after external Zed routing on macOS when the race guard permits, without AX permission or changing core sync results.

**Files:**
- Create: `src/focus.rs`
- Modify: `src/lib.rs`
- Modify: `src/zed.rs`
- Modify: `src/sync.rs`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `tests/sync_flow.rs`
- Modify: `tests/support/mod.rs`

**Dependencies:** Task 3 external Zed call boundary.

**Implementation notes:**
- Keep AppKit/Foundation dependencies under a macOS target section so Linux build/link behavior is unchanged.
- Define a small internal focus backend used around external Zed invocation; test behavior through fakes rather than real application activation.
- Capture the event-start frontmost app as late as possible before Zed launch.
- After Zed returns, restore only when the captured app differs and the current frontmost app bundle identifier is one of `dev.zed.Zed`, `dev.zed.Zed-Preview`, `dev.zed.Zed-Nightly`, or `dev.zed.Zed-Dev`; never match titles. Unknown/custom Zed bundles receive normal routing but no restoration.
- Run finalization on both Zed success and failure. Swallow capture, identity, and activation failures without notification or status changes. Document that a user switch after the frontmost check can still be overridden because AppKit provides no atomic compare-and-activate operation.
- Never run the backend for internal routes or external `focus=zed`.

**Test cases:**
- external terminal focus + original Ghostty + post-command Zed → restore Ghostty after the exact Zed call.
- external terminal focus + post-command observation is a non-Zed app → no restore.
- user switch simulated after a Zed observation but before activation → test/doc records that restoration may still win; no stronger guarantee is asserted.
- each stable/Preview/Nightly/Dev bundle ID → recognized; unknown/custom bundle → silent no-op.
- original app already Zed → no redundant restore.
- capture failure, identity failure, or activation failure → Zed result unchanged and no notification.
- Zed process failure after taking focus → restoration attempted, original Zed error still delivered.
- external focus=zed and all internal routes → no focus backend calls.
- Linux build/test seam → default focus=zed; explicit terminal rejected by Task 1 contract.

**Complete when:**
- Focus restoration cannot turn a successful Zed sync into failure or mask a Zed failure.
- No Accessibility/Automation API or permission is introduced.
- macOS automated tests pass locally and the hosted Linux CI test job passes before this task is completed. If hosted CI has not run, this task remains incomplete rather than treating Linux as verified.

**Validation:**
- Run: `cargo test --test sync_flow && cargo check --all-targets --all-features`
- Expected: deterministic focus-order/race/identity tests and the current macOS compilation path pass.
- Run: hosted `CI / Test (ubuntu-latest)` from `.github/workflows/ci.yml` after push.
- Expected: Linux compilation, Clippy, and all tests pass; Task 4 cannot complete and the plan cannot archive while this gate is unavailable or failing.

### Task 5: Complete Diagnostics and Public Documentation

**Covers:** R1-R5, R10, R14-R16, D8-D9

**Objective:** Make installed state, active mode, platform behavior, launch UX, and unavoidable Zed limitations discoverable without preserving obsolete task-first guidance.

**Files:**
- Modify: `src/doctor.rs`
- Modify: `src/setup.rs`
- Modify: `src/error.rs`
- Modify: `README.md`
- Modify: `docs/plans/2026-08-18-zerdr-mvp.md`
- Modify: `tests/setup_and_doctor.rs`
- Modify: `tests/cli_contract.rs`

**Dependencies:** Tasks 1-4 final contracts.

**Implementation notes:**
- Build on Task 1's remote static doctor path without redefining it. Local doctor must validate both final route strategies under the existing lifecycle lock and exact lease authority; remote doctor remains process/lock/cleanup-free.
- Internal live route output includes canonical dynamic anchor. External output includes focus policy and whether restoration is supported on the current platform.
- No-live guidance says run bare `zerdr`; task is described as an optional precise-anchor fallback.
- README quickstart is setup then bare launch, with concise internal/external examples and override syntax.
- Document that external `--existing` may add an unopened project to an eligible existing window, focus restoration is final-state best-effort with possible flash/Space movement, and Linux leaves Zed foreground.
- Supersede only conflicting original-plan statements; retain historical implementation records.
- Do not mention zed-herdr compatibility or conflict behavior.

**Test cases:**
- doctor + live internal v2/v1 route → one wrapper, mode internal, valid anchor.
- doctor + live external terminal/zed route → one wrapper, external focus policy, platform capability.
- doctor + malformed route, wrong PID, multiple leases, inaccessible session → existing blocking failures remain.
- no live local route/session → bare launcher guidance; stale routes removed per scope.
- each remote marker → static installation output plus boundary report; fixture bytes, lock paths, stale leases/routes, and process log remain unchanged.
- setup task/README/help commands → exact current CLI and five labels.
- README inspection → no required task-first launch, no public `zerdr herdr`, and explicit external/window/focus limitations.

**Complete when:**
- Doctor distinguishes both live modes without weakening stale cleanup.
- Public docs lead with `zerdr`, preserve optional task recovery, and make every accepted limitation explicit.
- Original MVP plan points to this active revision for conflicting launcher/external-terminal contracts.

**Validation:**
- Run: `cargo test --test setup_and_doctor && cargo test --test cli_contract`
- Expected: mode-aware doctor, setup migration, help, and documentation contracts pass.
- Run: `rg -n 'zerdr$|--mode|--anchor|--focus|Ghostty|iTerm|external|optional|existing window|new window|Space|Linux' README.md`
- Expected: README contains the agreed launch path, overrides, platform behavior, and limitations; plan-only matches do not satisfy this check.

### Task 6: Complete Regression and Real Internal/External E2E

**Covers:** R1-R16, D1-D9

**Objective:** Prove the revised candidate across automated gates and real Zed/Herdr terminal environments before any release decision.

**Files:**
- Modify: `docs/plans/2026-08-19-auto-terminal-routing.md` — record exact automated/manual outcomes and minor implementation differences.
- Modify: `docs/plans/2026-08-18-zerdr-mvp.md` — update revision completion/archive pointer only after all gates pass.

**Dependencies:** Tasks 1-5.

**Implementation notes:**
- Use disposable Git roots and close conflicting target windows before positive internal tests.
- Test external mode from real Ghostty on macOS; iTerm-specific testing is not required because restoration targets the event-start frontmost app generically.
- Record Zed, Herdr, zerdr, macOS, and terminal versions.
- Do not publish, tag, or mutate the Homebrew tap.

**Test cases:**
- Zed terminal + Git CWD + bare `zerdr` → internal route, existing anchored startup sync, one wrapper.
- Zed terminal + non-Git CWD → pre-spawn failure; explicit anchor fallback succeeds.
- Ghostty + bare `zerdr` → external route, Herdr appears, no startup Zed activation.
- External focus A/B in one Zed window → matching project/window ordered by Zed; Ghostty is frontmost again after each command on macOS.
- External focus C already in another Zed window → that window becomes Zed's selected/top window; Ghostty returns foreground.
- External focus unopened D → D opens or is added according to Zed's eligible-window behavior; no claim of mandatory new window.
- External `--focus zed` → Zed remains foreground.
- User switch completed early enough that the post-command check observes a non-Zed app → zerdr does not restore the captured terminal.
- User switch after a post-command Zed observation but before activation → accepted non-atomic race is recorded; restoration may win and no stronger guarantee is claimed.
- Target Zed window on another Space → transient movement is tolerated and final restoration outcome recorded.
- Same focus event/manual `sync` → repeated external Zed request.
- live external then internal launch, and live internal then external launch → candidate rejected; owner route/UI remains.
- optional Zed task after setup migration → explicit internal launch works and competing task candidate behavior remains correct.
- environment-fixture SSH/WSL/container rejection → covered automatically; no real remote E2E.
- Linux hosted CI → external sync tests pass with focus=zed default; GUI/window behavior remains unverified on Linux.

**Complete when:**
- All automated gates pass after the revision.
- Every applicable macOS E2E case has an observed result recorded in this plan.
- Linux GUI focus behavior remains explicitly unverified because it is out of the focus-restoration contract. Hosted macOS and Linux CI test jobs must pass; unavailable or failing hosted CI blocks completion/archive. Distribution-only checks that are unavailable remain explicitly marked N/A with their reason and do not imply publication readiness.
- No publication operation has occurred without separate approval.

**Validation:**
- Run: `cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-targets --all-features`
- Expected: no formatting/Clippy failures and all revised plus existing tests pass.
- Run: `cargo metadata --no-deps --format-version 1 >/dev/null && cargo package --allow-dirty --no-verify --list >/dev/null && actionlint .github/workflows/*.yml && git diff --check`
- Expected: metadata, package contents, workflows, and changed files remain valid.
- Run: the manual cases above on macOS with real Zed, Herdr, and Ghostty.
- Expected: mode selection, window routing, one-owner behavior, and final focus policy match the contracts and documented limitations.
- Run: hosted `CI / Test (macos-latest)` and `CI / Test (ubuntu-latest)` after push.
- Expected: both jobs pass before Task 6 completion or plan archival.

## Requirement Coverage

| Requirement / Decision | Task(s) | Verification |
|---|---|---|
| R1 | 1, 5, 6 | Bare CLI/removed-subcommand tests; README; real bare launch |
| R2 | 1, 6 | Zed/non-Zed environment matrix; real Zed and Ghostty launch |
| R3-R5 | 1, 4, 6 | Option conflict/CWD/platform tests; focus override E2E |
| R6, D3 | 3, 6 | Exact one-command external logs; existing/absent project E2E |
| R7, D4 | 2, 6 | No-startup-Zed fake log and Ghostty observation |
| R8 | 3, 6 | Repeat-event logs and manual sync E2E |
| R9, D5-D6 | 4, 6 | Fake frontmost/race/failure tests; macOS foreground observations |
| R10 | 1, 4-6 | Remote marker/doctor matrix, Linux seam, platform E2E boundaries |
| R11, D1-D2 | 1-3, 6 | Real admission races, PID authority, opposite-mode E2E |
| R12 | 1, 3 | Manual commands outside Zed under both route modes |
| R13 | 2-4 | Nonfatal startup/sync/restoration failure tests |
| R14, D8 | 2, 5, 6 | Plugin-only preflight, task migration, setup/manual task E2E |
| R15 | 5 | Internal/external/v1/malformed doctor fixtures |
| R16, D7 | 1-6 | v1 route fixtures, existing suites, full/package/workflow gates |
| D9 | 5 | README/doctor contain no third-party coordination contract |

## Final Validation

- [x] `cargo test --test cli_contract && cargo test --test state_and_bindings` — Passed: bare/mode/remote/route-schema contracts.
- [x] `cargo test --test herdr_wrapper && cargo test --test setup_and_doctor` — Passed: mode lifecycle, task migration, preflight, and doctor contracts.
- [x] `cargo test --test sync_flow` — Passed: exact internal/external routing, manual commands, authority, and focus restoration seams.
- [x] `cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-targets --all-features` — Passed with 85 tests.
- [x] `cargo metadata --no-deps --format-version 1 >/dev/null && cargo package --allow-dirty --no-verify --list >/dev/null` — Passed: target-specific dependency metadata and package contents.
- [x] `actionlint .github/workflows/*.yml && git diff --check` — Passed: workflows and changed files.
- [ ] Real macOS internal/external E2E checklist — Expected: every applicable Task 6 observation is recorded with exact versions.
- [ ] Hosted macOS/Linux CI — Expected: both test jobs pass after push; unavailable or failing CI blocks completion/archive.
- [x] Original MVP plan points to this revision; historical conflicting contracts are marked superseded.
- [x] Requirement Coverage has no unmapped requirement or decision.
- [x] The plan and actual changed-file set agree, including `objc2-app-kit` plus its transitive lockfile entries and the test support seams.
- [x] No release, tag, or Homebrew tap mutation occurred without explicit approval.
- [ ] After every item above succeeds, update the original MVP plan pointer to the archived path and move this file unchanged in name to `docs/plans/archived/2026-08-19-auto-terminal-routing.md`.

## Risks and Open Questions

### Risks

- Zed CLI window placement for an unopened external target depends on Zed's eligible-window behavior and can change across Zed versions; doctor/help capability checks and real E2E remain required.
- macOS application activation is cooperative. Focus restoration may fail, flash, or move between Spaces; this is accepted and silent by contract.
- Identifying post-command Zed by application identity must cover supported Zed release channels without falling back to window-title matching.
- An in-place binary update while an old internal wrapper is live requires v1 route read compatibility because plugin events execute the newly installed executable against the old owner's route.
- Remote/container detection is necessarily marker-based. The implementation must test and document its known markers without claiming detection of every possible container runtime.

### Open Questions

None. Changes to the CLI contract, route schema, focus policy, supported environments, setup preflight, or wrapper cardinality require user confirmation before implementation proceeds.
