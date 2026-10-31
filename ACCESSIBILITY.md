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
| Last reviewed | 2026-10-31 |

## What it emits that people read

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
- A refusal names the resource and what is missing: the capability it needs, the argument
  (a write without `calendar=` says writes always address a named calendar, with an example), or for an
  unknown period, every period form that works. Calendar access denied on macOS says where to
  grant it (System Settings › Privacy & Security › Calendars).

These are followed by reading the source; the tests below pin some of them.

## How it is tested

- `src/lib.rs` unit tests: `freebusy_drops_free_events_but_detail_keeps_them` (the word `busy`,
  `· free` in detail), `an_all_free_period_reads_free` (`(free)`),
  `detail_shows_titles_and_freebusy_hides_them`, `a_multi_day_event_blocks_every_day_it_spans`,
  `single_day_events_stay_one_line`, `an_unknown_period_is_rejected_at_the_grammar`, and
  `the_config_resource_reports_or_guides` (the missing-config refusal names the file to create).
- **No automated accessibility checker, browser or assistive technology has been used on this
  output.**

## Known limitations

Found by reading the source on 2026-10-31:

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
what was tested, with what, and what was not.

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
