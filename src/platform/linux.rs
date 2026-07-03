//! Linux backend for personal contexts — **not implemented yet**.
//!
//! The crate builds on Linux; the personal resources resolve to a clear
//! "not supported yet" error. A future backend would source contacts via LDAP /
//! Active Directory, calendar via CalDAV, or the desktop's Evolution Data Server
//! over D-Bus.

pub const NAME: &str = "Linux";
pub const SUPPORTED: bool = false;

pub fn contacts() -> Option<String> {
    None
}

pub fn calendars() -> Option<Result<Vec<super::CalendarInfo>, String>> {
    None
}

pub fn create_calendar(_name: &str, _account: Option<&str>) -> Option<Result<String, String>> {
    None
}

pub fn events(
    _start_epoch: i64,
    _end_epoch: i64,
    _calendar: Option<&str>,
) -> Option<Result<Vec<super::EventInfo>, String>> {
    None
}

pub fn create_event(
    _calendar: &str,
    _title: &str,
    _start_epoch: i64,
    _end_epoch: i64,
    _all_day: bool,
    _location: Option<&str>,
    _source_uid: Option<&str>,
) -> Option<Result<String, String>> {
    None
}

pub fn delete_event(
    _calendar: &str,
    _uid: &str,
    _window_start: i64,
    _window_end: i64,
) -> Option<Result<String, String>> {
    None
}
