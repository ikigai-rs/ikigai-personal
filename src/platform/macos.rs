//! macOS backend for personal contexts.
//!
//! Calendar enumeration and creation are REAL EventKit (via the `objc2`
//! bindings), including the TCC permission request — the first real slice of
//! the backend. The contacts/calendar-read functions below still return
//! clearly-labelled sample data; replacing them with EventKit/Contacts reads
//! is the next slice (P1 of the Brian-Busy plan).

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

// ---- real EventKit: calendar enumeration + creation --------------------------

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_event_kit::{EKAuthorizationStatus, EKCalendar, EKEntityType, EKEventStore, EKSource};
use objc2_foundation::{NSError, NSString};

/// An event store with calendar access ensured — asks TCC on first use (the
/// prompt is attributed to the hosting terminal/app; a denial says how to fix
/// it in System Settings). Created per call: cheap, and nothing Objective-C
/// crosses a thread boundary.
fn store() -> Result<Retained<EKEventStore>, String> {
    let status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
    let store = unsafe { EKEventStore::new() };
    if status == EKAuthorizationStatus::FullAccess {
        return Ok(store);
    }
    if status == EKAuthorizationStatus::NotDetermined {
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        let block = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
            let _ = tx.send(granted.as_bool());
        });
        let block_ptr = &*block as *const _ as *mut _;
        unsafe { store.requestFullAccessToEventsWithCompletion(block_ptr) };
        return match rx.recv_timeout(std::time::Duration::from_secs(120)) {
            Ok(true) => Ok(store),
            Ok(false) => Err(
                "calendar access was denied — grant it under System Settings › Privacy \
                 & Security › Calendars"
                    .to_string(),
            ),
            Err(_) => Err("timed out waiting for the calendar-access prompt".to_string()),
        };
    }
    Err(
        "calendar access is denied or restricted — enable it for this terminal/app under \
         System Settings › Privacy & Security › Calendars"
            .to_string(),
    )
}

/// Every event calendar the OS knows, with the account (source) each lives on.
pub fn calendars() -> Option<Result<Vec<super::CalendarInfo>, String>> {
    Some(calendars_impl())
}

fn calendars_impl() -> Result<Vec<super::CalendarInfo>, String> {
    let store = store()?;
    let list = unsafe { store.calendarsForEntityType(EKEntityType::Event) };
    let mut out = Vec::new();
    for i in 0..list.count() {
        let cal = list.objectAtIndex(i);
        let title = unsafe { cal.title() }.to_string();
        let account = unsafe { cal.source() }
            .map(|s| unsafe { s.title() }.to_string())
            .unwrap_or_else(|| "(no account)".to_string());
        out.push(super::CalendarInfo { title, account });
    }
    Ok(out)
}

/// Create a new event calendar named `name` on `account` (an EventKit source
/// title, e.g. "iCloud"); with no account, prefers a source named iCloud, then
/// the system default. Returns a confirmation naming the account used.
pub fn create_calendar(name: &str, account: Option<&str>) -> Option<Result<String, String>> {
    Some(create_calendar_impl(name, account))
}

fn create_calendar_impl(name: &str, account: Option<&str>) -> Result<String, String> {
    let store = store()?;

    // Refuse a duplicate rather than silently making "Name (2)".
    for existing in calendars_impl()? {
        if existing.title == name {
            return Err(format!(
                "a calendar named \"{name}\" already exists (on {})",
                existing.account
            ));
        }
    }

    // Pick the source: by title when asked; else prefer iCloud; else default.
    let sources = unsafe { store.sources() };
    let mut chosen: Option<Retained<EKSource>> = None;
    for i in 0..sources.count() {
        let source = sources.objectAtIndex(i);
        let title = unsafe { source.title() }.to_string();
        let is_match = match account {
            Some(wanted) => title.eq_ignore_ascii_case(wanted),
            None => title.eq_ignore_ascii_case("icloud"),
        };
        if is_match {
            chosen = Some(source);
            break;
        }
    }
    if chosen.is_none() {
        if let Some(wanted) = account {
            return Err(format!(
                "no calendar account named \"{wanted}\" — see urn:personal:calendars for \
                 the accounts in play"
            ));
        }
        chosen =
            unsafe { store.defaultCalendarForNewEvents() }.and_then(|cal| unsafe { cal.source() });
    }
    let Some(source) = chosen else {
        return Err("no usable calendar account found".to_string());
    };
    let account_title = unsafe { source.title() }.to_string();

    let calendar =
        unsafe { EKCalendar::calendarForEntityType_eventStore(EKEntityType::Event, &store) };
    unsafe {
        calendar.setTitle(&NSString::from_str(name));
        calendar.setSource(Some(&source));
        store
            .saveCalendar_commit_error(&calendar, true)
            .map_err(|e| format!("saving calendar failed: {e}"))?;
    }
    Ok(format!("created calendar \"{name}\" on {account_title}"))
}
