# Thread Title Marker Implementation Plan

> **For implementers:** Execute tasks in order unless dependencies allow otherwise. Mark a task complete only after its validation succeeds. Reflect minor implementation differences in the relevant task. Ask the user before changing requirements, Out of Scope, or public contracts.

## Problem Statement

The in-pane banner (typed shell comment) turned out to render badly in practice: the text
arrives before the shell starts, so it echoes twice, and fish's syntax highlighting paints
the comment red — it reads like an error. The user wants the "is this a Herdr pane?"
marker moved to the Zed agent panel's thread title instead, and the shell left alone.

## Goal

- Threads attached to Herdr show a sidebar title of the form
  `[herdr] <Display> - <detail>` (e.g. `[herdr] Claude - コード内の重複パターン洗い出し`),
  driven only by core Herdr data — no dependency on the herdr-agent-context plugin.
- The typed-into-the-shell banner is removed entirely.

## Out of Scope

- Reading herdr-agent-context metadata tokens (`agent_context_*`): it is an optional
  plugin not all users install. When it is installed, agents' own titles already carry
  the session name, so the display improves without zerdr knowing about the plugin.
- Herdr-side labels (`tab rename` / `pane rename`).
- Restoring the native leading-glyph icon promotion (lost by the `[herdr] ` prefix;
  verified acceptable: for terminal threads Zed's only dynamic indicator is the
  bell-driven notification dot, which is title-independent).
- The status lines, pane memory/restore, bell, and lease behavior (unchanged).

## Requirements and Decisions

### Requirements

- **R1:** The shell banner is removed: no `pane send-text` / `pane send-keys` calls, no
  `send_banner`, no `ZERDR_TEST_SEND_TEXT_EXIT` hook, and the banner tests are deleted.
  Freshly created panes receive no injected input of any kind.
- **R2:** The OSC title for an attached agent becomes `[herdr] <Display> - <detail>`:
  - `<Display>` maps the Herdr agent kind to a friendly name: `pi` → `Pi`,
    `claude` → `Claude`; any other kind is shown with its first letter uppercased.
  - `<detail>` is `terminal_title_stripped`, except that for kind `pi` a leading
    `π - ` is stripped (pi prefixes its own titles with it). When the title is empty
    or absent, `<detail>` falls back to the workspace label, then the workspace id.
- **R3:** A plain-shell pane (no agent) shows `[herdr] <workspace label>` (id when the
  label is unknown). When an agent later appears in the pane, the title switches to the
  R2 form on the next poll (existing monitor behavior).
- **R4:** Title updates stay live: the existing poll loop re-derives the label each tick
  and emits only on change; the settle-bell behavior is untouched.
- **R5:** README reflects the new title format and drops the banner description.

### Implementation Decisions

- **D1:** The marker moves entirely into the OSC title; the pane content is never
  touched. Confirmed against Zed source: the only cost of a `[herdr] ` prefix is the
  loss of leading-glyph icon promotion and of glyph carry-over on manual rename; the
  bell notification dot and all other rendering are title-independent.
- **D2:** Kind display names live in one small function beside `emit_title`
  (`pi`/`claude` special-cased, capitalize-first fallback); the `π - ` strip applies
  only to kind `pi`. No plugin token names appear anywhere in the code.
- **D3:** Everything else added by the banner task (herdr send wrappers, fake hook,
  tests) is reverted rather than kept dormant — dead write-paths into user shells are
  not worth carrying.

### Contracts

- Title for an agent pane: `[herdr] {display} - {detail}` exactly (single spaces, the
  literal `[herdr]` marker, ` - ` separator).
- Title for a shell pane: `[herdr] {workspace label or id}`.
- Emitted verbatim via OSC 0 as before; dedup-on-change unchanged.

## Current Context

### Confirmed

- Real `herdr agent list` output (user-provided): kind values are `pi` / `claude`;
  pi titles look like `π - <session name> - <dir>` in `terminal_title_stripped`; claude's
  `terminal_title_stripped` is the clean conversation title (its `✳ ` spinner appears
  only in the unstripped `terminal_title`). zerdr already parses
  `terminal_title_stripped` into `AgentInfo.title` and maps empty to `None`
  (src/herdr.rs `string_field(...).filter(|title| !title.is_empty())`).
- `emit_title` (src/thread.rs) currently emits `title.or(label).unwrap_or(workspace_id)`
  verbatim; `AgentInfo.kind` is empty for synthesized shell panes and carries the kind
  otherwise; the monitor re-polls `agent_get_for` and calls `emit_title` each tick.
- Banner code to remove: `send_banner` + `single_line` (src/thread.rs),
  `pane_send_text_for` / `pane_send_keys_for` (src/herdr.rs), the
  `ZERDR_TEST_SEND_TEXT_EXIT` branch (tests/support/mod.rs), tests
  `a_failing_banner_warns_but_does_not_block_the_attach`,
  `banner_text_never_contains_control_characters`,
  `create_with_a_kind_writes_the_banner_before_the_agent_starts`, and the banner
  assertions inside the new-tab / create / attach / restore tests.
- Zed-side behavior verified in zed-industries/zed source (terminal-thread rows have one
  icon slot; running/error badges never apply; notification dot is bell-driven and
  title-independent).

### Assumptions

- Exact capitalization helper behavior for multi-word or non-ASCII kinds is free
  (first-character uppercase is enough; kinds are herdr-validated lowercase names).

## File Structure

- Modify: `src/thread.rs` — `emit_title` composes the marker format; delete
  `send_banner`/`single_line` and both call sites.
- Modify: `src/herdr.rs` — delete `pane_send_text_for` / `pane_send_keys_for`.
- Test: `tests/support/mod.rs` — delete the `ZERDR_TEST_SEND_TEXT_EXIT` branch
  (`ZERDR_TEST_PANE_GET_MISSING_IDS` stays; the restore feature uses it).
- Test: `tests/thread_flow.rs` — delete banner tests/assertions; retarget every OSC
  assertion to the new format; add kind-mapping and pi-strip cases.
- Modify: `README.md` — new title format; remove the banner paragraph.

## Testing Decisions

- **Test seam:** unchanged (compiled binary + fake herdr fixtures, OSC payloads on
  stdout).
- **Behavior:** exact OSC payload strings for: claude agent, pi agent with `π - `
  prefix, unknown kind, empty title fallback, shell pane; absence of any `send-text` in
  the log anywhere.
- **Avoid:** asserting on the fake's unrelated log lines; over-specifying the
  capitalization helper beyond the mapped kinds and one fallback case.

## Progress

- [x] Task 1: Title marker and banner removal
- [x] Task 2: Documentation

## Tasks

### Task 1: Title marker and banner removal

**Covers:** R1, R2, R3, R4, D1, D2, D3

**Objective:** every zerdr-attached thread is recognizable by its `[herdr] ...` sidebar
title, and nothing is ever typed into a pane's shell.

**Files:**
- Modify: `src/thread.rs`, `src/herdr.rs`
- Test: `tests/thread_flow.rs`, `tests/support/mod.rs`

**Dependencies:** none

**Implementation notes:**
- `emit_title` derives the label from the polled `AgentInfo` each tick: kind empty →
  `[herdr] {label-or-id}`; kind present → `[herdr] {display(kind)} - {detail}` with the
  pi strip and the empty-title fallback per R2. Dedup and bell logic untouched.
- Fixture titles in tests use realistic values (e.g. pi: `π - 施策を進める - mog-app`,
  claude: `コード内の重複パターン洗い出し`). `Fixture::agent()` in tests/thread_flow.rs
  hardcodes `"agent": "pi"`, so it needs a kind parameter (or a sibling helper) to
  produce the claude / unknown-kind fixtures, and every existing OSC assertion that
  used a bare title or label moves to the `[herdr] ...` form.

**Test cases:**
- claude agent titled `コード内の重複パターン洗い出し` → OSC payload exactly
  `[herdr] Claude - コード内の重複パターン洗い出し`.
- pi agent titled `π - 施策を進める - mog-app` → `[herdr] Pi - 施策を進める - mog-app`.
- unknown kind `codex` titled `t` → `[herdr] Codex - t`.
- agent with empty title → `[herdr] {Display} - {workspace label}`.
- plain-shell pane (new tab) → `[herdr] {workspace label}`; on reattach the same.
- title change across polls still emits once per change and the settle bell still fires
  (existing `titles_are_emitted_once_per_change...` retargeted).
- the whole thread_flow log never contains `send-text` (checked in the creation tests).

**Complete when:**
- All OSC assertions use the new format; banner code paths and tests are gone;
  `rg -n "send-text|send_banner|SEND_TEXT_EXIT" src/ tests/` only matches the
  restore-unrelated leftovers (expected: no matches).
- Validation succeeds.

**Validation:**
- Run: `cargo test --test thread_flow --test herdr_wrapper --all-features && cargo test --all-targets --all-features`
- Expected: all tests pass (herdr_wrapper included for the fake-timing budget).

**Result:** Done. `emit_title` + `display_kind` + `strip_kind_prefix` in src/thread.rs;
`Fixture::agent_of_kind` added; new `titles_carry_the_herdr_marker_and_kind_display_names`
test covers claude / pi-strip / unknown-kind. One deviation from the "Complete when"
grep: the negative assertions the plan itself requires (`!log.contains("send-text")` in
the creation/attach/restore tests) necessarily contain the string "send-text", so the
grep matches those three test lines and nothing else — no banner code remains
(`rg "send_banner|single_line|pane_send"` has zero matches). Full suite green
(200 tests at --test-threads=4), fmt/clippy clean.

### Task 2: Documentation

**Covers:** R5

**Objective:** README matches the new marker behavior.

**Files:**
- Modify: `README.md`

**Dependencies:** Task 1

**Implementation notes:**
- Replace the banner sentence with the title-marker description
  (`[herdr] Claude - <会話タイトル>`-style example); note that the agent's own title is
  shown after the marker, so tools that enrich agent titles improve the display
  automatically (without naming the plugin as a dependency).
- The existing sentence claiming zerdr mirrors the title "exactly as the agent sets it
  ... including Zed promoting a leading spinner glyph to the row icon" (README.md:50)
  becomes false with the prefix and must be rewritten to describe the marker format
  and drop the glyph-promotion claim.

**Test cases:**
- N/A (prose); grep below.

**Complete when:**
- `rg -n "# zerdr:" README.md` has no matches; `rg -n "\[herdr\]" README.md` shows the
  new format; the "exactly as the agent sets it" / glyph-promotion sentence is gone;
  validation succeeds.

**Validation:**
- Run: `rg -n '\[herdr\]' README.md && rg -cn '# zerdr:' README.md; true`
- Expected: the marker format documented, the banner text gone.

**Result:** Done. The banner sentence and the "exactly as the agent sets it" /
glyph-promotion claim are gone; the title-marker paragraph documents both formats and
the enrichment note without naming the plugin. Validation greps confirm.

## Requirement Coverage

| Requirement / Decision | Task | Verification |
|---|---|---|
| R1 | Task 1 | banner code/tests removed; no `send-text` in any log |
| R2 | Task 1 | exact OSC assertions for claude / pi / unknown kind / empty title |
| R3 | Task 1 | shell-pane and reattach OSC assertions |
| R4 | Task 1 | retargeted dedup + bell test |
| R5 | Task 2 | README grep |
| D1–D3 | Task 1 | code review: no pane input calls remain, no plugin tokens |

## Final Validation

- [x] `cargo fmt --all -- --check` — Expected: no diff — clean
- [x] `cargo clippy --all-targets --all-features -- -D warnings` — Expected: no warnings — clean
- [x] `cargo test --all-targets --all-features` — Expected: all tests pass — 200 tests green
      (run at `--test-threads=4`; GitHub CI green on macOS and Ubuntu)
- [x] Manual check (user's machine, after `cargo install --path . --locked --force`):
      threads show `[herdr] Pi - ...` / `[herdr] Claude - ...` titles that follow the
      agents' own titles; a fresh shell pane shows `[herdr] <workspace>`; nothing is
      typed into the pane's shell anymore. — ユーザーが実機で確認済み。
- [x] Requirement Coverage に未対応項目がない — R1–R5 / D1–D3 すべて Task と検証に対応済み。
- [x] 計画と実際の変更内容が整合している — 実装コミット 07d4286, c3bf4a2, a3dd936、独立レビュー 2 回 APPROVED。
- [x] 上記のすべてが成功した後、計画を同名のまま `docs/plans/archived/` へ移した

**Independent review:** two rounds by a separate reviewer context. Round 1: APPROVED with
one low correctness note (a prefix-only pi title of exactly "π - " would leave a dangling
`[herdr] Pi - ` instead of falling back) and one informational process note. The low
finding was fixed in a3dd936 (`.filter(|detail| !detail.is_empty())` + a test case).
Round 2: APPROVED, no remaining issues. Implementation commits: 07d4286, c3bf4a2,
a3dd936 (base 9af1db1), all pushed, CI green.

## Risks and Open Questions

- Pi keeps its `- <dir>` suffix inside the detail (pi's own title format); accepted.
- If an agent kind ever sets titles prefixed with its own marker other than pi's
  `π - `, the detail will show it verbatim; per-kind strips can be added later.
- No open questions.
