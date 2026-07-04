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
    // ONE store per thread, reused: EventKit populates a fresh store's calendar
    // cache asynchronously, so rapid-fire store creation (a derivation pass
    // makes many calls) can race it into a briefly-incomplete calendar list —
    // seen live as "no calendar named X" for a calendar that exists. Apple's
    // guidance is to hold a long-lived store; per-thread keeps it !Send-safe.
    thread_local! {
        static STORE: std::cell::RefCell<Option<Retained<EKEventStore>>> =
            const { std::cell::RefCell::new(None) };
    }
    if let Some(existing) = STORE.with(|cell| cell.borrow().clone()) {
        return Ok(existing);
    }
    let created = fresh_store()?;
    STORE.with(|cell| *cell.borrow_mut() = Some(created.clone()));
    Ok(created)
}

fn fresh_store() -> Result<Retained<EKEventStore>, String> {
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

/// The events in `[start_epoch, end_epoch)` (unix seconds), optionally filtered
/// to one calendar by title, sorted by start. Real EventKit.
pub fn events(
    start_epoch: i64,
    end_epoch: i64,
    calendar: Option<&str>,
) -> Option<Result<Vec<super::EventInfo>, String>> {
    Some(events_impl(start_epoch, end_epoch, calendar))
}

fn rfc3339_local(epoch: f64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(epoch as i64, 0)
        .single()
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| epoch.to_string())
}

fn events_impl(
    start_epoch: i64,
    end_epoch: i64,
    calendar: Option<&str>,
) -> Result<Vec<super::EventInfo>, String> {
    use objc2_foundation::{NSArray, NSDate};

    let store = store()?;
    let start = NSDate::dateWithTimeIntervalSince1970(start_epoch as f64);
    let end = NSDate::dateWithTimeIntervalSince1970(end_epoch as f64);

    // The calendar filter: nil = all event calendars.
    let filtered: Option<Retained<NSArray<EKCalendar>>> = match calendar {
        Some(wanted) => {
            let all = unsafe { store.calendarsForEntityType(EKEntityType::Event) };
            let mut keep = Vec::new();
            for i in 0..all.count() {
                let cal = all.objectAtIndex(i);
                if unsafe { cal.title() }
                    .to_string()
                    .eq_ignore_ascii_case(wanted)
                {
                    keep.push(cal);
                }
            }
            if keep.is_empty() {
                return Err(format!(
                    "no calendar named \"{wanted}\" — see urn:personal:calendars"
                ));
            }
            Some(NSArray::from_retained_slice(&keep))
        }
        None => None,
    };

    let predicate = unsafe {
        store.predicateForEventsWithStartDate_endDate_calendars(&start, &end, filtered.as_deref())
    };
    let found = unsafe { store.eventsMatchingPredicate(&predicate) };

    let mut out = Vec::new();
    for i in 0..found.count() {
        let event = found.objectAtIndex(i);
        // Identity round-trip: events this system CREATED carry their source
        // identity (urn:event:{uid}) in the URL field — prefer it, so a written
        // event reads back as the same graph subject and diffs converge.
        let url_uid = unsafe { event.URL() }
            .and_then(|u| u.absoluteString())
            .map(|s| s.to_string())
            .and_then(|u| u.strip_prefix("urn:event:").map(str::to_string));
        let store_uid = unsafe { event.calendarItemExternalIdentifier() }
            .map(|s| s.to_string())
            .unwrap_or_else(|| unsafe { event.calendarItemIdentifier() }.to_string());
        let title = unsafe { event.title() }.to_string();
        let calendar = unsafe { event.calendar() }
            .map(|c| unsafe { c.title() }.to_string())
            .unwrap_or_else(|| "(no calendar)".to_string());
        let start = rfc3339_local(unsafe { event.startDate().timeIntervalSince1970() });
        let end = rfc3339_local(unsafe { event.endDate().timeIntervalSince1970() });
        // One EKEvent per OCCURRENCE, but occurrences of a recurring event share
        // the store UID — qualify by date so each occurrence is its own subject.
        let uid = url_uid.unwrap_or_else(|| {
            super::occurrence_uid(&store_uid, unsafe { event.hasRecurrenceRules() }, &start)
        });
        let location = unsafe { event.location() }.map(|s| s.to_string());
        let alerts = read_alerts(&event);
        out.push(super::EventInfo {
            uid,
            title,
            calendar,
            start,
            end,
            all_day: unsafe { event.isAllDay() },
            location,
            alerts,
        });
    }
    out.sort_by(|a, b| a.start.cmp(&b.start));
    Ok(out)
}

/// The event's relative alarms as minutes-before-start, sorted (absolute-date
/// alarms are skipped — the graph speaks offsets).
fn read_alerts(event: &objc2_event_kit::EKEvent) -> Vec<u32> {
    let Some(alarms) = (unsafe { event.alarms() }) else {
        return Vec::new();
    };
    let mut minutes: Vec<u32> = (0..alarms.count())
        .map(|i| alarms.objectAtIndex(i))
        .map(|alarm| unsafe { alarm.relativeOffset() })
        .filter(|offset| *offset < 0.0)
        .map(|offset| (-offset / 60.0).round() as u32)
        .collect();
    minutes.sort_unstable();
    minutes.dedup();
    minutes
}

/// Find one calendar by title (writes address a NAMED calendar, always).
fn calendar_named(store: &EKEventStore, title: &str) -> Result<Retained<EKCalendar>, String> {
    let all = unsafe { store.calendarsForEntityType(EKEntityType::Event) };
    for i in 0..all.count() {
        let cal = all.objectAtIndex(i);
        if unsafe { cal.title() }
            .to_string()
            .eq_ignore_ascii_case(title)
        {
            return Ok(cal);
        }
    }
    Err(format!(
        "no calendar named \"{title}\" — see urn:personal:calendars"
    ))
}

/// Create an event in a named calendar. `source_uid` (when given) is written to
/// the event's URL as `urn:event:{uid}` — the identity round-trip that lets a
/// later read recognize the event as the same graph subject.
#[allow(clippy::too_many_arguments)]
pub fn create_event(
    calendar: &str,
    title: &str,
    start_epoch: i64,
    end_epoch: i64,
    all_day: bool,
    location: Option<&str>,
    source_uid: Option<&str>,
    alerts: &[u32],
) -> Option<Result<String, String>> {
    Some(create_event_impl(
        calendar,
        title,
        start_epoch,
        end_epoch,
        all_day,
        location,
        source_uid,
        alerts,
    ))
}

#[allow(clippy::too_many_arguments)]
fn create_event_impl(
    calendar: &str,
    title: &str,
    start_epoch: i64,
    end_epoch: i64,
    all_day: bool,
    location: Option<&str>,
    source_uid: Option<&str>,
    alerts: &[u32],
) -> Result<String, String> {
    use objc2_event_kit::{EKAlarm, EKEvent, EKSpan};
    use objc2_foundation::{NSDate, NSURL};

    let store = store()?;
    let target = calendar_named(&store, calendar)?;
    let event = unsafe { EKEvent::eventWithEventStore(&store) };
    unsafe {
        event.setTitle(Some(&NSString::from_str(title)));
        event.setStartDate(Some(&NSDate::dateWithTimeIntervalSince1970(
            start_epoch as f64,
        )));
        event.setEndDate(Some(&NSDate::dateWithTimeIntervalSince1970(
            end_epoch as f64,
        )));
        event.setAllDay(all_day);
        if let Some(location) = location {
            event.setLocation(Some(&NSString::from_str(location)));
        }
        if let Some(uid) = source_uid {
            let url = NSURL::URLWithString(&NSString::from_str(&format!("urn:event:{uid}")));
            event.setURL(url.as_deref());
        }
        for minutes in alerts {
            let alarm = EKAlarm::alarmWithRelativeOffset(-(f64::from(*minutes) * 60.0));
            event.addAlarm(&alarm);
        }
        event.setCalendar(Some(&target));
        store
            .saveEvent_span_commit_error(&event, EKSpan::ThisEvent, true)
            .map_err(|e| format!("saving event failed: {e}"))?;
    }
    Ok(format!("created \"{title}\" in {calendar}"))
}

/// Delete the event with `uid` (URL-carried source identity, or the
/// occurrence-qualified store uid) from a named calendar, searching
/// `[window_start, window_end)`.
pub fn delete_event(
    calendar: &str,
    uid: &str,
    window_start: i64,
    window_end: i64,
) -> Option<Result<String, String>> {
    Some(delete_event_impl(calendar, uid, window_start, window_end))
}

fn delete_event_impl(
    calendar: &str,
    uid: &str,
    window_start: i64,
    window_end: i64,
) -> Result<String, String> {
    use objc2_event_kit::EKSpan;
    use objc2_foundation::{NSArray, NSDate};

    let store = store()?;
    let target = calendar_named(&store, calendar)?;
    let cals = NSArray::from_retained_slice(&[target]);
    let predicate = unsafe {
        store.predicateForEventsWithStartDate_endDate_calendars(
            &NSDate::dateWithTimeIntervalSince1970(window_start as f64),
            &NSDate::dateWithTimeIntervalSince1970(window_end as f64),
            Some(&cals),
        )
    };
    let found = unsafe { store.eventsMatchingPredicate(&predicate) };
    for i in 0..found.count() {
        let event = found.objectAtIndex(i);
        let url_uid = unsafe { event.URL() }
            .and_then(|u| u.absoluteString())
            .map(|s| s.to_string())
            .and_then(|u| u.strip_prefix("urn:event:").map(str::to_string));
        let store_uid = unsafe { event.calendarItemExternalIdentifier() }
            .map(|s| s.to_string())
            .unwrap_or_else(|| unsafe { event.calendarItemIdentifier() }.to_string());
        let start = rfc3339_local(unsafe { event.startDate().timeIntervalSince1970() });
        let occurrence =
            super::occurrence_uid(&store_uid, unsafe { event.hasRecurrenceRules() }, &start);
        if url_uid.as_deref() == Some(uid) || occurrence == uid {
            let title = unsafe { event.title() }.to_string();
            unsafe {
                store
                    .removeEvent_span_commit_error(&event, EKSpan::ThisEvent, true)
                    .map_err(|e| format!("removing event failed: {e}"))?;
            }
            return Ok(format!("removed \"{title}\" from {calendar}"));
        }
    }
    Err(format!(
        "no event with uid {uid} in {calendar} within the window"
    ))
}

/// Run `on_change` whenever the event store reports ANY change — an edit in
/// Calendar.app, an accepted invitation, an iCloud sync from another device.
/// The observer lives on a dedicated thread whose runloop services the
/// framework's delivery; the store, queue, and token are intentionally leaked
/// (process-lifetime observation). The freshness source that makes calendar
/// reactions event-driven instead of polled.
pub fn observe_store(on_change: Box<dyn Fn() + Send>) -> Option<()> {
    std::thread::spawn(move || {
        use objc2_foundation::{
            NSDefaultRunLoopMode, NSNotificationCenter, NSOperationQueue, NSPort, NSRunLoop,
        };

        let Ok(store) = store() else { return };
        let queue = NSOperationQueue::new();
        let block = block2::RcBlock::new(
            move |_notification: core::ptr::NonNull<objc2_foundation::NSNotification>| {
                on_change();
            },
        );
        // object: None — observe the notification regardless of WHICH store
        // instance posts it (this process holds thread-local stores).
        let token = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(objc2_event_kit::EKEventStoreChangedNotification),
                None,
                Some(&queue),
                &block,
            )
        };
        // Keep everything alive for the life of the process.
        std::mem::forget(token);
        std::mem::forget(store);
        std::mem::forget(queue);
        eprintln!("ikigai: calendar observer active");
        // A runloop with NO sources exits `run()` immediately — attach a dummy
        // Mach port so the loop blocks and services the framework's delivery.
        let runloop = NSRunLoop::currentRunLoop();
        unsafe { runloop.addPort_forMode(&NSPort::port(), NSDefaultRunLoopMode) };
        runloop.run();
        eprintln!("ikigai: calendar observer runloop exited (unexpected)");
    });
    Some(())
}
