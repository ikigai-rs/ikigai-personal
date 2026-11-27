//! Put one event by its stable uid: update it in place when the named calendar
//! already holds it, create it when it does not.
//!
//! The decision lives here, over the small [`EventStore`] seam, so it is tested
//! against an in-memory store on every platform. The macOS backend implements the
//! seam over EventKit, whose calls an agent shell or a CI runner cannot make (TCC).

/// Everything a put states about one event. A put REPLACES what it states: an
/// absent location, description or alert list clears the stored one, because the
/// caller's record (a class session, an org heading) is the truth and the calendar
/// entry is its projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventWrite<'a> {
    pub title: &'a str,
    /// Unix seconds.
    pub start: i64,
    /// Unix seconds, exclusive.
    pub end: i64,
    pub all_day: bool,
    pub location: Option<&'a str>,
    pub description: Option<&'a str>,
    /// Minutes before start, sorted and deduplicated.
    pub alerts: &'a [u32],
}

/// One stored event a put found by its uid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found<H> {
    /// The store's handle for the event, passed back to [`EventStore::update`].
    pub handle: H,
    /// Its current start and end, unix seconds.
    pub start: i64,
    pub end: i64,
}

/// What a put did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PutOutcome {
    /// No event in the calendar carried the uid, so one was made.
    Created,
    /// The event carrying the uid was rewritten in place. `moved` is true when its
    /// start or end changed.
    Updated { moved: bool },
}

/// The store operations a put needs. Each call names the calendar, since writes
/// always address a named calendar.
pub trait EventStore {
    type Handle;
    /// The event in `calendar` carrying `uid` whose occurrence falls in
    /// `[window_start, window_end)`, if any.
    fn find(
        &mut self,
        calendar: &str,
        uid: &str,
        window_start: i64,
        window_end: i64,
    ) -> Result<Option<Found<Self::Handle>>, String>;
    /// Make a new event carrying `uid`.
    fn create(&mut self, calendar: &str, uid: &str, event: &EventWrite<'_>) -> Result<(), String>;
    /// Rewrite the found event with `event`, keeping its identity.
    fn update(&mut self, handle: Self::Handle, event: &EventWrite<'_>) -> Result<(), String>;
}

/// A day either side of a stated start: the narrow window a write or delete with
/// a known start searches first.
pub fn near(start: i64) -> (i64, i64) {
    (start - 86_400, start + 2 * 86_400)
}

/// The wide window searched when the event is not near its new start (it moved):
/// a year back to two years ahead of `now`, inside EventKit's four-year limit on
/// one predicate.
pub fn wide(now: i64) -> (i64, i64) {
    (now - 366 * 86_400, now + 2 * 366 * 86_400)
}

/// Put `event` into `calendar` under `uid`. The store is searched near the new
/// start first (an edit that keeps the date), then across [`wide`] (a move); only
/// when neither finds the uid is a new event made. An event moved further than the
/// wide window reaches is not found, and a second copy is made: the limit is
/// stated in the README.
pub fn put_event<S: EventStore>(
    store: &mut S,
    calendar: &str,
    uid: &str,
    event: &EventWrite<'_>,
    now: i64,
) -> Result<PutOutcome, String> {
    let (near_start, near_end) = near(event.start);
    let found = match store.find(calendar, uid, near_start, near_end)? {
        Some(found) => Some(found),
        None => {
            let (wide_start, wide_end) = wide(now);
            store.find(calendar, uid, wide_start, wide_end)?
        }
    };
    match found {
        Some(found) => {
            let moved = found.start != event.start || found.end != event.end;
            store.update(found.handle, event)?;
            Ok(PutOutcome::Updated { moved })
        }
        None => {
            store.create(calendar, uid, event)?;
            Ok(PutOutcome::Created)
        }
    }
}

/// The confirmation sentence for a put.
pub fn confirmation(outcome: PutOutcome, title: &str, calendar: &str) -> String {
    match outcome {
        PutOutcome::Created => format!("created \"{title}\" in {calendar}"),
        PutOutcome::Updated { moved: true } => {
            format!("updated \"{title}\" in {calendar}: moved to its new time")
        }
        PutOutcome::Updated { moved: false } => format!("updated \"{title}\" in {calendar}"),
    }
}

#[cfg(test)]
pub(crate) mod memory {
    //! An in-memory [`EventStore`] for tests: no operating system calendar.
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Stored {
        pub calendar: String,
        pub uid: String,
        pub title: String,
        pub start: i64,
        pub end: i64,
        pub all_day: bool,
        pub location: Option<String>,
        pub description: Option<String>,
        pub alerts: Vec<u32>,
    }

    #[derive(Default)]
    pub struct Memory {
        pub events: Vec<Stored>,
        /// Every window `find` was asked about, in order.
        pub searches: Vec<(i64, i64)>,
        /// When set, every call fails with this text.
        pub fail: Option<String>,
    }

    impl Memory {
        fn check(&self) -> Result<(), String> {
            self.fail.clone().map_or(Ok(()), Err)
        }
    }

    fn fill(stored: &mut Stored, event: &EventWrite<'_>) {
        stored.title = event.title.to_string();
        stored.start = event.start;
        stored.end = event.end;
        stored.all_day = event.all_day;
        stored.location = event.location.map(str::to_string);
        stored.description = event.description.map(str::to_string);
        stored.alerts = event.alerts.to_vec();
    }

    impl EventStore for Memory {
        type Handle = usize;

        fn find(
            &mut self,
            calendar: &str,
            uid: &str,
            window_start: i64,
            window_end: i64,
        ) -> Result<Option<Found<usize>>, String> {
            self.check()?;
            self.searches.push((window_start, window_end));
            Ok(self
                .events
                .iter()
                .enumerate()
                .find(|(_, e)| {
                    e.calendar.eq_ignore_ascii_case(calendar)
                        && e.uid == uid
                        && e.start < window_end
                        && e.end > window_start
                })
                .map(|(i, e)| Found {
                    handle: i,
                    start: e.start,
                    end: e.end,
                }))
        }

        fn create(
            &mut self,
            calendar: &str,
            uid: &str,
            event: &EventWrite<'_>,
        ) -> Result<(), String> {
            self.check()?;
            let mut stored = Stored {
                calendar: calendar.to_string(),
                uid: uid.to_string(),
                title: String::new(),
                start: 0,
                end: 0,
                all_day: false,
                location: None,
                description: None,
                alerts: Vec::new(),
            };
            fill(&mut stored, event);
            self.events.push(stored);
            Ok(())
        }

        fn update(&mut self, handle: usize, event: &EventWrite<'_>) -> Result<(), String> {
            self.check()?;
            fill(&mut self.events[handle], event);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::memory::Memory;
    use super::*;

    const NOW: i64 = 1_796_000_000; // 2026-11-29
    const HOUR: i64 = 3_600;
    const DAY: i64 = 86_400;

    fn session(start: i64) -> EventWrite<'static> {
        EventWrite {
            title: "Session 3: graphs",
            start,
            end: start + HOUR,
            all_day: false,
            location: Some("Room 2"),
            description: Some("Join link in the class page"),
            alerts: &[15],
        }
    }

    #[test]
    fn a_first_put_creates_and_a_second_updates_in_place() {
        let mut store = Memory::default();
        let first = put_event(&mut store, "Teaching", "s3", &session(NOW + DAY), NOW).unwrap();
        assert_eq!(first, PutOutcome::Created);
        let again = put_event(&mut store, "Teaching", "s3", &session(NOW + DAY), NOW).unwrap();
        assert_eq!(again, PutOutcome::Updated { moved: false });
        assert_eq!(store.events.len(), 1, "a repeated put never duplicates");
    }

    #[test]
    fn a_moved_session_moves_its_entry_instead_of_duplicating_it() {
        let mut store = Memory::default();
        put_event(&mut store, "Teaching", "s3", &session(NOW + DAY), NOW).unwrap();
        store.searches.clear();
        // Moved a week later: not near the new start, so the wide window finds it.
        let moved = session(NOW + 8 * DAY);
        let outcome = put_event(&mut store, "Teaching", "s3", &moved, NOW).unwrap();
        assert_eq!(outcome, PutOutcome::Updated { moved: true });
        assert_eq!(store.events.len(), 1);
        assert_eq!(store.events[0].start, NOW + 8 * DAY);
        assert_eq!(store.events[0].end, NOW + 8 * DAY + HOUR);
        assert_eq!(
            store.searches,
            vec![near(NOW + 8 * DAY), wide(NOW)],
            "near the new start first, then the wide window"
        );
    }

    #[test]
    fn an_edit_on_the_same_day_is_found_without_the_wide_search() {
        let mut store = Memory::default();
        put_event(&mut store, "Teaching", "s3", &session(NOW + DAY), NOW).unwrap();
        store.searches.clear();
        let later_that_day = session(NOW + DAY + 2 * HOUR);
        let outcome = put_event(&mut store, "Teaching", "s3", &later_that_day, NOW).unwrap();
        assert_eq!(outcome, PutOutcome::Updated { moved: true });
        assert_eq!(store.searches, vec![near(NOW + DAY + 2 * HOUR)]);
    }

    #[test]
    fn a_put_replaces_every_field_it_states() {
        let mut store = Memory::default();
        put_event(&mut store, "Teaching", "s3", &session(NOW + DAY), NOW).unwrap();
        let bare = EventWrite {
            title: "Session 3: graphs (renamed)",
            location: None,
            description: None,
            alerts: &[],
            ..session(NOW + DAY)
        };
        put_event(&mut store, "Teaching", "s3", &bare, NOW).unwrap();
        let stored = &store.events[0];
        assert_eq!(stored.title, "Session 3: graphs (renamed)");
        assert_eq!(stored.location, None, "an absent location clears it");
        assert_eq!(stored.description, None);
        assert!(stored.alerts.is_empty());
    }

    #[test]
    fn the_uid_is_scoped_to_the_named_calendar() {
        let mut store = Memory::default();
        put_event(&mut store, "Teaching", "s3", &session(NOW + DAY), NOW).unwrap();
        let outcome = put_event(&mut store, "Home", "s3", &session(NOW + DAY), NOW).unwrap();
        assert_eq!(outcome, PutOutcome::Created);
        assert_eq!(store.events.len(), 2);
        assert_eq!(
            store.events[0].calendar, "Teaching",
            "the first is untouched"
        );
    }

    #[test]
    fn a_store_failure_is_reported_and_nothing_is_written() {
        let mut store = Memory {
            fail: Some("calendar access is denied".into()),
            ..Memory::default()
        };
        let err = put_event(&mut store, "Teaching", "s3", &session(NOW), NOW).unwrap_err();
        assert_eq!(err, "calendar access is denied");
        assert!(store.events.is_empty());
    }

    #[test]
    fn confirmations_say_what_happened_in_words() {
        assert_eq!(
            confirmation(PutOutcome::Created, "S", "Teaching"),
            "created \"S\" in Teaching"
        );
        assert_eq!(
            confirmation(PutOutcome::Updated { moved: false }, "S", "Teaching"),
            "updated \"S\" in Teaching"
        );
        assert_eq!(
            confirmation(PutOutcome::Updated { moved: true }, "S", "Teaching"),
            "updated \"S\" in Teaching: moved to its new time"
        );
    }
}
