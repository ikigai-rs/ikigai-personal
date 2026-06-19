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

pub use imp::{availability, calendar, contacts, NAME, SUPPORTED};
