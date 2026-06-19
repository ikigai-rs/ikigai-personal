//! Fallback backend for any other target (e.g. wasm, BSD) — **not implemented**.
//!
//! The crate builds; the personal resources resolve to a clear "not supported"
//! error naming the current platform.

pub const NAME: &str = std::env::consts::OS;
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
