//! Per-OS backend dispatch. Exactly one `imp` is compiled, selected by the
//! target operating system. Each backend exposes `NAME`, `SUPPORTED`, and the
//! `contacts` / `calendar` / `availability` functions; an unimplemented platform
//! returns `None` so the resource resolves to a clear "not supported" error.
//!
//! Adding a real backend for a platform means filling in its file — the seam and
//! the rest of the crate don't change.

#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(target_os = "windows", path = "windows.rs")]
#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows", target_os = "linux")),
    path = "unsupported.rs"
)]
mod imp;

pub use imp::{calendars, contacts, create_calendar, events, NAME, SUPPORTED};

/// One native calendar, as the OS reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarInfo {
    /// The calendar's title, e.g. "Brian" / "Bosatsu".
    pub title: String,
    /// The account (EventKit source) it lives on, e.g. "iCloud".
    pub account: String,
}

/// One calendar event, normalized from the OS store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventInfo {
    /// The event's stable external identifier (the iCal UID on iCloud) — the
    /// natural key events are skolemized under (`urn:event:{uid}`), the same
    /// identity the org ingestion records.
    pub uid: String,
    /// The event title.
    pub title: String,
    /// The native calendar it lives on, e.g. "Bosatsu".
    pub calendar: String,
    /// Start/end as RFC 3339 local timestamps.
    pub start: String,
    pub end: String,
    /// An all-day event (start/end are dates, times are midnight).
    pub all_day: bool,
    /// The location, when set.
    pub location: Option<String>,
}
