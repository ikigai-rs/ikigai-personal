//! Fallback backend for any other target (e.g. wasm, BSD) — **not implemented**.
//!
//! The crate builds; the personal resources resolve to a clear "not supported"
//! error naming the current platform.

pub const NAME: &str = std::env::consts::OS;
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
