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

pub use imp::{
    calendars, contacts, create_calendar, create_event, delete_event, events, observe_store, NAME,
    SUPPORTED,
};

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

/// The skolemization id for one fetched event: recurring events share one
/// store UID across occurrences, so each occurrence is qualified by its start
/// date — otherwise every occurrence collapses onto one graph subject (an
/// "event" with several dtstarts). Mirrors the org side's `…-YYYY-MM-DD`.
// Only the macOS backend calls this today; other platforms' event backends
// will when they land.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn occurrence_uid(store_uid: &str, recurring: bool, start_rfc3339: &str) -> String {
    if recurring {
        let date = start_rfc3339.split('T').next().unwrap_or(start_rfc3339);
        format!("{store_uid}-{date}")
    } else {
        store_uid.to_string()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn recurring_occurrences_get_distinct_uids() {
        let a = super::occurrence_uid("UID1", true, "2026-06-29T10:00:00-07:00");
        let b = super::occurrence_uid("UID1", true, "2026-06-30T10:00:00-07:00");
        assert_ne!(a, b);
        assert_eq!(a, "UID1-2026-06-29");
        assert_eq!(
            super::occurrence_uid("UID2", false, "2026-07-11T19:00:00-07:00"),
            "UID2"
        );
    }
}
