//! Windows backend for personal contexts — **not implemented yet**.
//!
//! The crate builds on Windows; the personal resources resolve to a clear
//! "not supported yet" error. A future backend would source contacts and
//! calendar via the Windows Runtime (`Windows.ApplicationModel.Contacts` /
//! `Appointments`), Microsoft Graph, or Active Directory in an enterprise
//! context.

pub const NAME: &str = "Windows";
pub const SUPPORTED: bool = false;

pub fn contacts() -> Option<String> {
    None
}

pub fn calendar() -> Option<String> {
    None
}

pub fn availability() -> Option<String> {
    None
}
