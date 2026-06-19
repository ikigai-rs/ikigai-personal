//! macOS backend for personal contexts.
//!
//! TODO: replace the sample data below with real Contacts (`Contacts.framework`)
//! and calendar (`EventKit`) access via the `objc2` bindings — including the TCC
//! permission requests. For now it returns clearly-labelled sample data so the
//! resources resolve and the shape of each representation is demonstrable.

pub const NAME: &str = "macOS";
pub const SUPPORTED: bool = true;

const SAMPLE_NOTE: &str = "(sample data — Contacts/EventKit integration pending)";

pub fn contacts() -> Option<String> {
    Some(format!(
        "personal contacts {SAMPLE_NOTE}\n\n  \
         Ada Lovelace    <ada@analytical.engine>\n  \
         Alan Turing     <alan@bombe.uk>\n  \
         Grace Hopper    <grace@cobol.mil>\n"
    ))
}

pub fn calendar() -> Option<String> {
    Some(format!(
        "personal calendar — detailed {SAMPLE_NOTE}\n\n  \
         09:00-09:30  Standup (3 attendees)\n  \
         11:00-12:00  Design review: resolution fabric\n  \
         14:00-15:00  1:1 with Grace\n  \
         18:30-19:30  Dinner — Ada\n"
    ))
}

pub fn availability() -> Option<String> {
    // The free/busy PROJECTION: busy blocks only, no titles or attendees.
    Some(format!(
        "availability — free/busy {SAMPLE_NOTE}\n\n  \
         09:00-09:30  busy\n  \
         11:00-12:00  busy\n  \
         14:00-15:00  busy\n  \
         18:30-19:30  busy\n"
    ))
}
