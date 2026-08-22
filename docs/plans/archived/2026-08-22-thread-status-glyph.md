# Thread Title Status Glyph Implementation Plan

> **For implementers:** Execute tasks in order unless dependencies allow otherwise. Mark a task complete only after its validation succeeds. Reflect minor implementation differences in the relevant task. Ask the user before changing requirements, Out of Scope, or public contracts.

## Problem Statement

Zed's agent panel sidebar shows a live status for plain terminal threads running Claude Code (spinner while working, settled glyph when done), but a thread attached to a Herdr pane through `zerdr thread` shows only the static terminal icon. The status users see for plain threads is not a Zed feature reading agent state: Zed's `terminal_title_prefix` / `split_leading_icon_char` (Zed PR #57983) promote a leading "non-alphanumeric run + whitespace" from the terminal title into the sidebar row icon, and Claude Code happens to animate such a glyph in its own title. zerdr currently destroys that signal twice: it reads Herdr's `terminal_title_stripped` (glyph already removed) and emits `[herdr] Claude - <detail>`, whose leading `[h` fails Zed's prefix rule. Pi never puts a status glyph in its title at all (`π - <session> - <cwd>` only, and `π` is alphanumeric), so pi threads show no status even in plain terminals.

## Goal

`zerdr thread` emits agent-pane titles with a leading status glyph that Zed promotes into the sidebar row icon:

- When the agent's own raw title carries a glyph prefix (Claude Code's spinner frames), pass that glyph through, giving exact parity with a plain terminal thread.
- Otherwise (pi, agents without title glyphs), synthesize the glyph from Herdr's `agent_status` using Herdr's own Symbols-style indicator set, so the Zed sidebar mirrors the Herdr UI.

## Out of Scope

- Any Zed-side changes (the glyph promotion already exists in Zed 1.16.x).
- A pi extension that adds native title spinners (possible independently via pi's extension `setTitle` API; not part of zerdr).
- Changing the poll cadence, bell behavior, `[herdr]` marker format, or the `<detail>` derivation (including pi's `π - ` folding).
- Animating the fallback glyph (no spinner frames synthesized by zerdr; one static glyph per status).
- Status glyphs for plain shell panes (no agent, nothing to report).
- New configuration surface (no flag or env var to choose glyph sets).

## Requirements and Decisions

### Requirements

- **R1:** Titles emitted for agent panes (non-empty `kind`) always begin with a status glyph and one space: `<glyph> [herdr] <Kind> - <detail>` (and `<glyph> [herdr] <Kind> - <fallback>` when the detail falls back). The `[herdr]` marker and everything after it are unchanged from current behavior.
- **R2:** If the agent's raw terminal title (Herdr `terminal_title`) has a leading prefix per Zed's rule — a run of one or more non-whitespace, non-alphanumeric characters followed by at least one whitespace character and a non-empty remainder — that run is passed through as the glyph, verbatim, followed by a single space. Examples: `⠐ Fix tests` → `⠐`; `✳ Fix` → `✳`; `>>> go` → `>>>`. Non-matches: `π - review` (π is alphanumeric), `✳Thinking` (no whitespace), `Fix tests`, empty/absent raw title. Tightened during review: a run containing a control character is also disqualified, because the glyph is re-emitted inside zerdr's own OSC 0 sequence, which a stray ESC or BEL would corrupt.
- **R3:** When R2 does not match, the glyph comes from Herdr's `agent_status` using Herdr's Symbols indicator set: `working` → `◐`, `blocked` → `×`, `done` → `✓`, `idle` → `○`, anything else (including `unknown`) → `·`.
- **R4:** Plain shell panes (empty `kind`) keep the current glyph-less title `[herdr] <workspace>`.
- **R5:** Title deduplication compares the full emitted label including the glyph: a glyph-only change (spinner frame advance, status transition with unchanged detail) re-emits the title; an unchanged label does not. Bell emission on working→settled transitions is unchanged.
- **R6:** Every emitted agent-pane title must itself satisfy Zed's prefix rule so exactly the glyph becomes the row icon and `[herdr] …` remains the visible row title. This holds by construction of R1–R3 (all fallback glyphs are non-alphanumeric symbols; the pass-through run contains no alphanumerics by definition).
- **R7:** The README "Terminal threads" section describes the leading status glyph: native agent glyph passed through when present, Herdr-style status symbol otherwise, and that Zed shows it as the thread row icon.

### Implementation Decisions

- **D1:** Pass-through wins over synthesis. It gives Claude Code threads pixel parity with plain terminal threads (animated braille frames while working, `✳` when settled) and automatically benefits any future agent that decorates its own title.
- **D2:** The fallback set is Herdr's **Symbols** style (`× ◐ ✓ ○ ·` from herdr `src/ui/status.rs::state_icon_symbol`), fixed, regardless of the user's Herdr `status_indicators` configuration. Herdr's default Dots style distinguishes states by color (`●` red/yellow/teal), and Zed renders title glyphs without color, so Dots would collapse working/blocked/done into identical dots.
- **D3:** Status→glyph mapping follows Herdr's own semantics: Herdr reports `done` for idle-but-unseen and `idle` for idle-and-seen, mapped to `✓` and `○` exactly as Herdr's UI does.
- **D4:** Glyph freshness is bounded by the existing poll interval (`ZERDR_THREAD_POLL_MS`, default 2000 ms). Spinner frames advance at most once per poll; this is accepted and not tuned in this change.
- **D5:** `AgentInfo` gains a raw-title field parsed from Herdr's `terminal_title`; the existing `title` field keeps its `terminal_title_stripped`-first precedence so `<detail>` derivation (including `strip_kind_prefix`) is untouched.

### Contracts

- Emitted OSC 0 payloads (`\x1b]0;<label>\x07`):
  - Agent pane: `<glyph> [herdr] <Kind> - <detail>` — glyph is either the raw-title prefix run (R2) or one of `◐ × ✓ ○ ·` (R3).
  - Shell pane: `[herdr] <workspace>` (unchanged).
- `AgentInfo` (src/herdr.rs) adds `pub raw_title: Option<String>` populated from the `terminal_title` field of Herdr agent JSON, `None` when absent or empty. Existing fields keep their meaning.
- Glyph selection is a pure function of `(agent_status, raw_title)`; it performs no I/O and lives beside `emit_title` in src/thread.rs.

## Current Context

### Confirmed

- Zed main (and 1.16.x) promotes a title prefix into the sidebar row icon: `crates/agent_ui/src/terminal_thread_metadata_store.rs::terminal_title_prefix` (leading non-alphanumeric run + whitespace; any leading alphanumeric aborts) and `crates/sidebar/src/sidebar.rs::split_leading_icon_char` / `pick_icon_glyph` (strips one bracket pair, collapses repeated chars, takes the first grapheme). Display-only; no agent identification is derived from the glyph. Landed via Zed PR #57983.
- Terminal threads have no other status channel in Zed: sidebar `TerminalEntry` carries only `has_notification` (set by `TerminalEvent::Bell`, shown as a blue dot when the thread is not visible). The ACP spinner/warning icons are not wired to terminals.
- Claude Code (2.1.220) animates its terminal title with leading spinner glyphs (frame sets `·✢✳✶✻✽` / braille observed live, `✳` when settled). pi 0.83.0 sets `${APP_TITLE} - ${sessionName} - ${cwdBasename}` only on session events (`dist/modes/interactive/interactive-mode.js::updateTerminalTitle`) — no status glyph ever.
- Herdr (0.8.2, herdrdev/herdr) reports per-agent `terminal_title` (raw, glyph intact) and `terminal_title_stripped` in `agent list` / `agent get` JSON, plus `agent_status` ∈ working/idle/done/blocked/unknown. Verified live: a working claude pane reported `terminal_title: "⠐ Zed herdr…"` with the stripped variant lacking the glyph.
- Herdr's status indicator glyphs (`src/ui/status.rs::state_icon_symbol`): Dots = `● ● ● ○ ·`, Symbols = `× ◐ ✓ ○ ·` for blocked/working/done/idle/unknown; `StatusIndicatorStyle` defaults to Dots and the user's config.toml does not override it.
- zerdr today: `parse_agent` (src/herdr.rs:653) reads `["terminal_title_stripped", "terminal_title"]` into `title`; `emit_title` (src/thread.rs:522) emits `[herdr] {Kind} - {detail}` / `[herdr] {workspace}` and dedups on the label; the Monitor polls every `ZERDR_THREAD_POLL_MS` (default 2000 ms) and rings the bell on working→{idle,done,blocked}.
- Existing integration tests assert exact OSC payloads via `OSC_PREFIX` in tests/thread_flow.rs; fixtures (`agent_of_kind`, `agent_response`) currently supply only `terminal_title_stripped`.

### Assumptions

- Herdr may omit `terminal_title` for some panes; absence simply means no pass-through (R3 applies). No Herdr version gate is needed because the field is additive and optional.
- Prefix runs arriving from Herdr titles normally contain no control characters; the R2 character-class check nevertheless rejects a run carrying one (review hardening), so a hostile title cannot corrupt the emitted OSC sequence through the glyph. The `<detail>` portion keeps its pre-existing, unsanitized behavior.

## File Structure

- Modify: `src/herdr.rs` — parse `terminal_title` into the new `AgentInfo::raw_title`.
- Modify: `src/thread.rs` — glyph selection helpers (raw-title prefix per R2, status mapping per R3) and `emit_title` composition per R1/R4.
- Modify: `tests/thread_flow.rs` — extend fixtures with a raw-title field; update existing OSC assertions for the new glyph; add pass-through, fallback, and transition cases.
- Modify: `tests/herdr_wrapper.rs` — assert `raw_title` parsing (present, absent) at the adapter seam.
- Modify: `README.md` — "Terminal threads" section, status glyph description (R7).

## Testing Decisions

- **Test seam:** the existing integration seam — `zerdr thread` run against the fake Herdr in tests/thread_flow.rs, asserting on emitted OSC 0 payloads in captured stdout. No new unit-test module; the repo verifies thread behavior at this boundary.
- **Behavior:** fixture agents gain a raw title; assertions pin the exact `\x1b]0;<glyph> [herdr] …` strings for pass-through, each fallback status, shell panes, and dedup/re-emit on glyph-only changes.
- **Prior art:** `titles_are_emitted_once_per_change_and_a_bell_marks_settling`, `titles_carry_the_herdr_marker_and_kind_display_names`, `an_empty_title_falls_back_to_the_workspace_label` in tests/thread_flow.rs.
- **Avoid:** asserting on helper function internals or poll timing beyond what the existing sequence-response fixture already models.

## Progress

- [x] Task 1: Status glyph in emitted thread titles
- [x] Task 2: README update

Tasks 1 and 2 ship as one commit: the README paragraph is inseparable from the behavior it documents.

## Tasks

### Task 1: Status glyph in emitted thread titles

**Covers:** R1, R2, R3, R4, R5, R6, D1, D2, D3, D5

**Objective:** `zerdr thread` emits `<glyph> [herdr] …` for agent panes — native glyph passed through when the raw title has one, Herdr Symbols-style status glyph otherwise — while shell panes and bell behavior stay as they are.

**Files:**
- Modify: `src/herdr.rs`
- Modify: `src/thread.rs`
- Modify: `tests/thread_flow.rs`
- Modify: `tests/herdr_wrapper.rs` (added during implementation: `raw_title` parse assertions in `agents_for` and `agent_get_for` tests)

**Dependencies:** none.

**Implementation notes:**
- Add `raw_title: Option<String>` to `AgentInfo`, filled from `terminal_title` (empty → `None`), in `parse_agent` and at the three construction sites in src/thread.rs that build placeholder `AgentInfo` values (shell/reattach/start fallbacks use `None`).
- Implement the R2 prefix rule to match Zed's `terminal_title_prefix` semantics exactly (see R2 examples; any alphanumeric before the first whitespace aborts, whitespace before any prefix character aborts, a prefix with no following whitespace or no remainder aborts). Normalize the emitted separator to a single ASCII space regardless of the whitespace run in the source title.
- Keep glyph selection pure and colocated with `emit_title`; `emit_title` prepends the glyph only when `kind` is non-empty. The existing `last` label dedup already gives R5 once the glyph is part of the label.
- The bell logic and `SETTLED_STATES` are untouched.
- Update fixture helpers (`agent`, `agent_of_kind`, `agent_response`) to also carry a raw title; default it to the stripped title (glyph-less) so existing scenarios exercise the R3 fallback, and let individual tests override it for pass-through cases.
- Existing assertions change mechanically: `idle` fixtures now expect `○ [herdr] …`, `working` fixtures `◐ [herdr] …`; the shell-pane expectations (`[herdr] checkout`) must stay glyph-less.

**Test cases:**
- Claude-kind agent, status `working`, raw title `⠐ fix tests`, stripped `fix tests` → emits `\x1b]0;⠐ [herdr] Claude - fix tests` (pass-through beats `◐`).
- Pi-kind agent, status `working`, raw title `π - review` (alphanumeric lead) → emits `◐ [herdr] Pi - review` (no pass-through).
- Raw title `✳Thinking` (no whitespace after run) → fallback glyph, not `✳`.
- Status sequence working → idle with unchanged detail → two OSC emissions: `◐ …` then `○ …` (glyph-only change re-emits; bell still rings once on settle).
- Statuses `done`, `blocked`, `unknown` → glyphs `✓`, `×`, `·` respectively.
- Empty-kind shell pane → `[herdr] checkout` with no glyph (existing tests keep passing unchanged).
- Identical consecutive polls (same status, same titles) → single OSC emission (dedup preserved).

**Complete when:**
- All listed test cases exist and pass alongside the updated existing tests.
- Shell-pane titles and status-line output are byte-identical to current behavior.
- `cargo clippy` and `cargo fmt` are clean.

**Validation:**
- Run: `cargo test --test thread_flow`
- Expected: all tests pass, including the new glyph cases.

### Task 2: README update

**Covers:** R7

**Objective:** The README's "Terminal threads" section tells users the sidebar title starts with a live status glyph and what it means.

**Files:**
- Modify: `README.md`

**Dependencies:** Task 1 (documents its behavior).

**Implementation notes:**
- Extend the existing paragraph that documents `[herdr] Pi - <title>` / `[herdr] Claude - <title>`: the title now leads with the agent's own spinner glyph when the agent provides one, otherwise one of Herdr's status symbols (`◐` working, `×` blocked, `✓` done, `○` idle, `·` unknown), and Zed displays that glyph as the thread's row icon. Keep the existing tone and length; English, matching the file.

**Test cases:**
- README describes both glyph sources and the shell-pane exception → verified by reading the diff.

**Complete when:**
- The section matches the implemented behavior, including the shell-pane exception.

**Validation:**
- Run: `git diff README.md`
- Expected: only the "Terminal threads" section changes; the described glyphs match R2/R3.

## Requirement Coverage

| Requirement / Decision | Task | Verification |
|---|---|---|
| R1 | Task 1 | Exact OSC assertions `<glyph> [herdr] …` for agent panes |
| R2 | Task 1 | Pass-through case (`⠐ fix tests`) and non-matches (`π - review`, `✳Thinking`) |
| R3 | Task 1 | Fallback cases for working/blocked/done/idle/unknown |
| R4 | Task 1 | Shell-pane assertions stay `[herdr] checkout` |
| R5 | Task 1 | working→idle re-emit case; identical-poll dedup case; bell assertion unchanged |
| R6 | Task 1 | By construction; pass-through/fallback glyph assertions pin the `<glyph><space>` shape |
| R7 | Task 2 | README diff review |
| D1 | Task 1 | Pass-through-beats-status test case |
| D2, D3 | Task 1 | Fallback glyph set assertions (`× ◐ ✓ ○ ·`) |
| D4 | — | No change to poll code; covered by existing sequence tests |
| D5 | Task 1 | `raw_title` used only for glyph; `<detail>` assertions unchanged |

## Final Validation

- [x] `cargo test --test thread_flow` — Expected: pass, including new glyph cases — passed (38 tests)
- [x] `cargo test --all-targets --all-features` — Expected: pass — passed (204 tests, 0 failures)
- [x] `cargo clippy --all-targets --all-features -- -D warnings` — Expected: no warnings — clean
- [x] `cargo fmt --all -- --check` — Expected: no diff — clean
- [x] Manual check: run `zerdr thread` in a Zed terminal thread against a Herdr pane running Claude Code and one running pi; the Zed sidebar row icon shows the claude spinner frames / `✳`, and `◐`/`○` for pi as it works and settles — confirmed by the user (first attempt ran a stale `~/.cargo/bin/zerdr` built before this change; after `cargo install --path . --locked` and opening a fresh thread, the glyphs display as specified, and the settle notification still shows the blue dot)
- [x] Requirement Coverage has no unaddressed rows
- [x] The plan matches the actual changes
- [x] After all of the above succeed, move this plan unchanged to `docs/plans/archived/`

## Implementation Record

- Commits: `a7e9f6a` feat(thread): lead thread titles with a status glyph (Tasks 1+2 and this plan); `1b70332` fix(thread): disqualify control characters from the title glyph run (review fix). Both pushed.
- Independent review (fresh-context reviewer, against this plan and `git diff 20cbadb..a7e9f6a`): no blocking/high findings. Two low findings — control characters could pass through the glyph run into the OSC payload, and the empty-remainder abort branch of `title_glyph_prefix` lacked a dedicated test — both fixed in `1b70332` (guard on `char::is_control()`, two new rows in `unusable_raw_prefixes_fall_back_to_the_status_glyph`). Re-review verdict: resolved, no new issues; `is_control()` (category Cc) cannot reject a legitimate visible glyph.

## Risks and Open Questions

- Herdr title polling granularity: glyph frames advance only per zerdr poll (default 2 s), so the claude spinner animates slower than in a plain terminal. Accepted (D4); `ZERDR_THREAD_POLL_MS` remains the tuning knob.
- Herdr builds that omit `terminal_title` silently degrade to the R3 fallback — acceptable and covered by the fallback tests.
- If Zed later changes its `terminal_title_prefix` rule, the pass-through predicate could drift from Zed's parser; the rule is pinned by tests here and would need a follow-up if Zed's parsing changes.
- The user's Herdr runs the default Dots indicator style; the sidebar glyphs intentionally use the Symbols set instead (D2). If seeing different symbols than the Herdr tab bar is confusing in practice, a follow-up could read Herdr's configured style.
