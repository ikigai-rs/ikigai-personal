# ikigai-personal

Personal contexts for [ikigai](https://github.com/ikigai-rs) — OS-backed
resources under `urn:personal:*`, resolved through the local operating system so
the data never leaves the machine. The calendar side is **real EventKit** on
macOS: reads, writes, and calendar management, capability-projected throughout.

| resource | verbs | what it is |
|----------|-------|------------|
| `urn:personal:calendar[:{period}]` | Source, Sink, Delete | events for a period; **write** events into a named calendar; **delete** by identity |
| `urn:personal:availability[:{period}]` | Source | the free/busy projection — busy blocks only, no detail |
| `urn:personal:calendars` | Source, Sink | the calendar collection: list every calendar with its account; **create** one |
| `urn:personal:calendar:config` | Source | the consolidated-view configuration the host loaded |
| `urn:personal:contacts` | Source | the local address book (still sample data — the next backend slice) |

## Periods, search, and faces

`{period}` is `today` · `tomorrow` · `week` (Monday-start) · `month` · `year` ·
a month name · `YYYY-MM` · `YYYY-MM-DD` · a range `YYYY-MM-DD..YYYY-MM-DD`
(end-inclusive — rolling windows that cross a year boundary are one period);
bare = `week`. `calendar=` restricts to one named calendar; `q=` searches
case-insensitively over titles and locations.

Three representations, **projected on the capability**:

- **detail text** — times, titles, calendars, locations (`…:read:detail`)
- **free/busy text** — busy blocks only (`…:read:freebusy`)
- **`as=text/turtle`** — the **skolemized event graph**: `urn:event:{uid}`
  subjects (no blank nodes), iCal RDF vocabulary, `ik:calendar` provenance,
  multi-valued `ik:alert` (minutes before start) — so calendars union and diff
  as ordinary graph set operations. Detail-gated: Turtle carries titles.

The capability wall has teeth: `q=` under a free/busy-only capability is denied
*before any platform call* — searching titles is reading them, and an attenuated
agent must not have a title-probing oracle. That's the data-minimization story,
live: *an agent books around your week without ever learning what you're doing.*

## Event identity (why diffs converge)

Every event is skolemized under a stable UID: the store's iCal UID, with
**recurring occurrences date-qualified** (`UID-2026-06-29` — occurrences share a
series UID in iCalendar, and each must be its own graph subject). Writes accept
a `uid=` which is stamped on the event's URL as `urn:event:{uid}`, and reads
**prefer** a URL-carried identity — so an event written from another system of
record reads back as the *same subject*, making derivation passes idempotent
and duplicate-free.

## Alarms

Writes accept `alert=` — friendly space/comma-separated tokens (`1h 1d`,
`30m,15`, bare numbers are minutes) — and each token becomes an `EKAlarm` that
fires that long before the event starts. Reads emit the alarms back as
multi-valued `ik:alert`, so alarms survive graph diffs: an org entry's
`:ALERT:` drawer line flows through a derivation pass onto the derived
calendar, and your phone rings.

## The consolidated-view configuration

`CalendarConfig` (hand-editable JSON a host conventionally loads from
`~/.config/ikigai/calendar.json`) names the derived household calendar, the
source allowlist, the capture inbox, and the account:

```json
{ "view": "Brian-Busy", "account": "iCloud",
  "sources": ["Brian", "Bosatsu"], "inbox": "Brian-New" }
```

`Sink urn:personal:calendars` with no arguments creates the configured view
calendar on the configured account.

## Platform support

| platform | status |
|----------|--------|
| **macOS** | real EventKit (objc2): list/create calendars, read/search/write/delete events, alarms, TCC full-access flow, one reused store per thread, store-change observation API |
| **Windows / Linux** | placeholders — build cleanly, resolve to a clear "not supported yet" error |

Adding a backend means filling in one file under `src/platform/` — the seam and
the rest of the crate don't change. Note macOS TCC attribution follows the
*hosting* process: terminal-launched processes prompt normally; a non-bundled
binary under another app's ancestry may be silently denied.

## Usage

```rust
use ikigai_core::Kernel;
use std::sync::Arc;

let config = ikigai_personal::CalendarConfig::from_json(&config_json).ok();
let kernel = Kernel::new(Arc::new(ikigai_personal::space(config)));
// source urn:personal:calendar:week calendar=Bosatsu
// source urn:personal:calendar:year q=dentist as=text/turtle
// sink   urn:personal:calendar calendar=Brian-Busy start=2026-07-11T19:00:00-07:00 alert=1h uid=… Dinner
```

Personal data is treated as a live fact and is **uncacheable**.

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at
your option.
