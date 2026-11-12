# Accessibility

What `ikigai-personal` emits that people read, the rules that output follows, how it is tested,
and how to report a barrier. It is not a conformance claim. Adapted from the
[ACCESSIBILITY.md template](https://github.com/mgifford/ACCESSIBILITY.md) (MIT).

| Field | Value |
|---|---|
| Target | WCAG 2.2 Level AA, for any page that presents this output; this repository writes text only (plain text, Turtle and JSON), no HTML or SVG |
| Status | Evaluation in progress. No conformance claim. |
| Owner | the maintainers of this repository |
| Report a barrier | [a GitHub issue](https://github.com/ikigai-rs/ikigai-personal/issues/new?labels=accessibility) with the `accessibility` label; anything security-sensitive through [private vulnerability reporting](https://github.com/ikigai-rs/ikigai-personal/security/advisories/new) |
| Last reviewed | 2026-11-12 |

## What it emits that people read

- **Documentation**: `README.md`, `AGENTS.md` and this file, read as Markdown on GitHub
  or in an editor.
- **Calendar text** (`src/lib.rs`): the detail face of `urn:personal:calendar[:{period}]`
  (`format_detail`: a `calendar — <period>` heading, then one line per event per day it covers,
  with date, time, title, `[calendar]`, `@ location` and `· free`), the free/busy face of the
  same resource and of `urn:personal:availability` (`format_freebusy`: date, time and the word
  `busy`), and the calendar list of `urn:personal:calendars` (`title  [account]`).
- **Write confirmations** from the macOS backend (`src/platform/macos.rs`): `created "<title>"
  in <calendar>`, `removed "<title>" from <calendar>`, `created calendar "<name>" on <account>`.
- **Contacts** (`urn:personal:contacts`): sample data today, headed `personal contacts (sample
  data — Contacts/EventKit integration pending)`.
- **Refusals**: the `Error` sentences in `src/lib.rs` (an unknown period, a missing capability, a
  calendar the configured view does not serve, a missing `calendar=` or title, no config loaded,
  an unsupported platform) and the backend's (calendar access denied, no such calendar or
  account, no event with that uid in the window). Two `ikigai:` lines on stderr from the calendar
  observer.
- **CalDAV helpers** (`src/caldav.rs`): XML request bodies, iCalendar text and credential
  refusals returned to callers; no network transport or page of their own.
- **Graph and config faces**: the Turtle event graph (`as=text/turtle`) and the config as JSON.
  People read these through other tools.

Not this repository's: the event titles, locations, notes, attendees and calendar names, which
are the calendar store's text shown as written; the operating system's own error text inside
`saving event failed: …`; and any page or terminal a host puts this text in.

## The rules that output follows

- Nothing is said by color or layout alone: a free event is marked with the word `free`, a busy
  block with the word `busy`, an empty period with `(no events)` or `(free)`, and an all-day
  event with `all-day`. No color is written.
- A multi-day event gets one line for every day it covers, so no day reads as free when it is
  not.
- Resource-level refusals describe what is missing: the capability it needs, the argument
  (a write without `calendar=` says writes always address a named calendar, with an example), or for an
  unknown period, every period form that works. Calendar access denied on macOS says where to
  grant it (System Settings › Privacy & Security › Calendars).

These rules guide changes; the tests below pin only the stated cases. Source inspection is
not an evaluation with assistive technology.

## How it is tested

Every named check has a result: Pass, Fail or Blocked, with evidence. A Pass covers only
that check, not overall accessibility or WCAG conformance. On 2026-11-12, before editing this
file, `cargo test --lib` passed all 33 tests on main commit
[`dd0f605`](https://github.com/ikigai-rs/ikigai-personal/commit/dd0f605403204ae6b6fa9c0a38f439d2eea97c8b),
using Cargo 1.99.1 and rustc 1.99.1 on macOS. The Pass rows below are `src/lib.rs` unit tests
included in that command. CI repeats these unit tests through
[the Rust workflow](.github/workflows/ci.yml); it does not repeat manual evaluations.

| Check | Result | Evidence |
|---|---|---|
| Busy and free status in words | Pass | `freebusy_drops_free_events_but_detail_keeps_them`: asserts `busy`, omission of a free birthday from busy blocks, and `· free` in detail. Does not test how those words or symbols are spoken. |
| All-free period | Pass | `an_all_free_period_reads_free`: asserts `(free)` for a period containing only a free event; does not test a host's presentation. |
| Detail fields and free/busy projection | Pass | `detail_shows_titles_and_freebusy_hides_them`: asserts title, `[calendar]` and `@ location` in detail, and their absence from busy output. Does not establish that punctuation is understandable with assistive technology. |
| Multi-day and all-day events | Pass | `a_multi_day_event_blocks_every_day_it_spans`: asserts four dated busy lines, `all-day`, the exclusive end, and repeated detail titles. Covers the fixture, not every date or time zone. |
| Single-day event lines | Pass | `single_day_events_stay_one_line`: asserts two busy lines with the expected time ranges. Does not test visual alignment or wrapping. |
| Unknown-period refusal | Pass | `an_unknown_period_is_rejected_at_the_grammar`: asserts `unknown period` in the error; does not evaluate the full guidance or its readability. |
| Config response and missing-config guidance | Pass | `the_config_resource_reports_or_guides`: parses the JSON view and asserts the missing-config error names `.config/ikigai/calendar.json`. Does not evaluate JSON navigation in a host. |
| Remaining output rules and end-to-end backend text | Blocked | No complete output audit or live EventKit session was performed for this update. The unit tests above do not verify every refusal, write confirmation, calendar list, contacts text, observer message, empty detail period, Turtle or CalDAV output for accessibility. |
| Documentation and host rendering in a browser or terminal | Blocked | No interactive rendering session was used for this update. Heading navigation, keyboard access, contrast, zoom, reflow and text spacing in GitHub, editors and consuming hosts remain unverified. This crate supplies no HTML or SVG interface. |
| Automated accessibility checker | Blocked | No browser or terminal accessibility checker has been run on this output; no checker harness is configured here. Rust unit tests are not an accessibility checker. |
| Manual keyboard | Blocked | No person performed a keyboard pass through documentation or a host displaying this output for this update. |
| Screen reader | Blocked | No screen reader session was used on this output; spoken order, punctuation and announcements remain unverified. |
| Screen magnifier | Blocked | No screen magnifier session was used on this output; readability and navigation under magnification remain unverified. |
| Speech input | Blocked | No speech-input session was used on this output or a consuming host. |
| Switch access | Blocked | No switch-access session was used on this output or a consuming host. |
| Braille display | Blocked | No braille display was used on this output; punctuation and line structure remain unverified. |
| Historical source review of limitations | Blocked | The 2026-10-31 source review below was not an assistive-technology run or a CI check. Its findings were rechecked against `format_detail`, `format_freebusy`, `day_lines` and the Delete argument handling on 2026-11-12, but their effect in a host remains unevaluated. |

## Known limitations

Found by reading the source on 2026-10-31 and rechecked on 2026-11-12; these are
source-level observations, not results from assistive technology:

- Times in the text faces carry no time zone. Each event is shown in the offset its calendar
  stored it in, so two lines can be in different zones and nothing says which.
- Dates are `YYYY-MM-DD` with no weekday, and times are a 24-hour `HH:MM` only.
- Event fields are told apart by spacing and punctuation (`[…]` for the calendar, `@` for the
  location, `·` before `free`), and `all-day` is padded with spaces to line up a column. A screen
  reader may read the symbols aloud or skip them, and the alignment assumes a fixed-width font.
- The Delete refusal for a missing uid says only `uid`, without saying what it is.
- All text is English, with no way to choose another language.

## Contributing

Read this file before changing any output listed above: the text faces, the confirmations, or
the refusals. Keep meaning in words, never in color or position alone. Say in the pull request
what was tested in a `Check | Result | Evidence` table: Pass, Fail or Blocked, with the
command or test, tool and version, or why it could not run. Include unrun checks as Blocked,
including assistive technology nobody used. A historical run CI does not repeat is Blocked
with its date; it is not a current Pass.

## Reporting a barrier

[Open an issue](https://github.com/ikigai-rs/ikigai-personal/issues/new?labels=accessibility)
with the `accessibility` label (the link sets it when GitHub lets you add labels; otherwise a
maintainer adds it): what you were doing, what happened, and your terminal or other tool and any
assistive technology, if you are comfortable sharing them. Anything security-sensitive goes
through [private vulnerability reporting](https://github.com/ikigai-rs/ikigai-personal/security/advisories/new).
**We acknowledge a report within 7 days**, and promise nothing further.

## Change history

| Date | Change |
|---|---|
| 2026-10-31 | First version. |
| 2026-11-12 | Record current unit-test evidence and explicit Blocked checks; clarify scope and evaluation limits. |
