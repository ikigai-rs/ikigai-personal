//! `ikigai-personal` — personal contexts for ikigai.
//!
//! Late-bound, OS-backed resources under `urn:personal:*` — your contacts,
//! calendar, and free/busy availability — resolved through the local operating
//! system rather than a remote service, so the data never leaves the machine.
//!
//! ## Platform support
//!
//! macOS is the first backend. **Windows and Linux are placeholders today**: the
//! crate *builds* on them, but the personal resources resolve to a clear
//! "not supported yet" error until their backends land (Windows: Microsoft Graph
//! / Active Directory; Linux: LDAP / CalDAV / D-Bus). The macOS backend currently
//! returns clearly-labelled **sample data** pending the real Contacts/EventKit
//! integration.
//!
//! ## Capability scoping
//!
//! Access is gated by `urn:cap:` capability scopes (see `ikigai-core`'s
//! `Capability`). `urn:personal:calendar` projects on the capability: a holder of
//! `urn:cap:personal:calendar:read:detail` sees full detail; one with only
//! `…:read:freebusy` sees busy blocks (no titles or attendees); one with neither
//! is denied. That's the data-minimization story — *"an agent books around my
//! week without ever learning what I'm doing"* — and it's why detail and free/busy
//! are one resource with two capability-scoped views rather than two resources.

mod platform;

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};
use ikigai_core::{
    ArgSpec, Description, EndpointSpace, Error, Exact, FnEndpoint, Invocation, ReprType,
    Representation, Result, UriTemplate, Verb,
};
use serde::Deserialize;

pub use platform::{CalendarInfo, EventInfo};

fn text_plain_utf8() -> ReprType {
    ReprType::new("text/plain").with_param("charset", "utf-8")
}

/// The `urn:cap:` read scope for a personal resource, e.g.
/// `read_scope("urn:personal:calendar", Some("detail"))` →
/// `urn:cap:personal:calendar:read:detail`.
fn read_scope(resource: &str, facet: Option<&str>) -> String {
    let nss = resource.strip_prefix("urn:").unwrap_or(resource);
    match facet {
        Some(facet) => format!("urn:cap:{nss}:read:{facet}"),
        None => format!("urn:cap:{nss}:read"),
    }
}

/// Wrap a backend result into a representation. Personal data is a *live* OS
/// fact, so it is deliberately uncacheable. A platform with no backend yet yields
/// a clear error rather than empty or stale data.
fn resolve(resource: &str, body: Option<String>) -> Result<Representation> {
    match body {
        Some(text) => Ok(Representation::new(text_plain_utf8(), text.into_bytes())),
        None => Err(Error::Endpoint(format!(
            "urn:personal:{resource} is not supported on {} yet — \
             personal contexts currently require macOS",
            platform::NAME
        ))),
    }
}

/// Error for a request the capability doesn't authorize.
fn denied(resource: &str, needs: &str) -> Error {
    Error::Endpoint(format!(
        "urn:personal:{resource} is not authorized — needs {needs}"
    ))
}

/// `contacts`: the local address book, gated on `urn:cap:personal:contacts:read`.
pub fn contacts() -> FnEndpoint {
    FnEndpoint::new("contacts", |inv: &Invocation<'_>| {
        let scope = read_scope("urn:personal:contacts", None);
        if !inv.capability.allows(&scope) {
            return Err(denied("contacts", &scope));
        }
        resolve("contacts", platform::contacts())
    })
    .with_description(
        Description::new("contacts")
            .title("Contacts")
            .summary("The local address book, resolved through the operating system.")
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .output("text/plain;charset=utf-8"),
    )
}

/// The `[start, end)` unix range (and a label) for a period name: `today`,
/// `tomorrow`, `week` (Monday-start, 7 days), `month`, a month name
/// (`june` — this year), `YYYY-MM`, or `YYYY-MM-DD`.
fn period_range(period: &str, today: NaiveDate) -> Result<(i64, i64, String)> {
    let day = |d: NaiveDate| (d, d + Duration::days(1), format!("{d}"));
    let (start, end, label) = match period {
        "today" => day(today),
        "tomorrow" => day(today + Duration::days(1)),
        "week" => {
            let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
            (
                monday,
                monday + Duration::days(7),
                format!("week of {monday}"),
            )
        }
        "month" => month_range(today.year(), today.month()),
        "january" | "february" | "march" | "april" | "may" | "june" | "july" | "august"
        | "september" | "october" | "november" | "december" => {
            let month = [
                "january",
                "february",
                "march",
                "april",
                "may",
                "june",
                "july",
                "august",
                "september",
                "october",
                "november",
                "december",
            ]
            .iter()
            .position(|m| *m == period)
            .expect("matched above") as u32
                + 1;
            month_range(today.year(), month)
        }
        other => {
            if let Ok(date) = other.parse::<NaiveDate>() {
                day(date)
            } else if let Some((y, m)) = other
                .split_once('-')
                .and_then(|(y, m)| Some((y.parse::<i32>().ok()?, m.parse::<u32>().ok()?)))
            {
                if !(1..=12).contains(&m) {
                    return Err(bad_period(other));
                }
                month_range(y, m)
            } else {
                return Err(bad_period(other));
            }
        }
    };
    Ok((local_midnight(start), local_midnight(end), label))
}

fn bad_period(period: &str) -> Error {
    Error::Endpoint(format!(
        "urn:personal:calendar:{period}: unknown period — try today, tomorrow, week, month, \
         a month name, YYYY-MM, or YYYY-MM-DD"
    ))
}

fn month_range(year: i32, month: u32) -> (NaiveDate, NaiveDate, String) {
    let start = NaiveDate::from_ymd_opt(year, month, 1).expect("valid month");
    let end = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .expect("valid month");
    (start, end, format!("{year}-{month:02}"))
}

fn local_midnight(date: NaiveDate) -> i64 {
    Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight"))
        .earliest()
        .map(|t| t.timestamp())
        .unwrap_or_default()
}

/// `HH:MM` from an RFC 3339 local timestamp (the formatting faces are compact).
fn hhmm(rfc3339: &str) -> &str {
    rfc3339
        .split_once('T')
        .map(|(_, t)| &t[..5.min(t.len())])
        .unwrap_or(rfc3339)
}

fn date_of(rfc3339: &str) -> &str {
    rfc3339.split_once('T').map(|(d, _)| d).unwrap_or(rfc3339)
}

/// The detailed text face: date, times, title, calendar, location.
fn format_detail(label: &str, events: &[EventInfo]) -> String {
    if events.is_empty() {
        return format!("calendar — {label}\n\n  (no events)\n");
    }
    let mut out = format!("calendar — {label}\n\n");
    for e in events {
        let when = if e.all_day {
            format!("{}  all-day    ", date_of(&e.start))
        } else {
            format!("{}  {}-{}", date_of(&e.start), hhmm(&e.start), hhmm(&e.end))
        };
        out.push_str(&format!("  {when}  {}  [{}]", e.title, e.calendar));
        if let Some(location) = &e.location {
            out.push_str(&format!("  @ {location}"));
        }
        out.push('\n');
    }
    out
}

/// The free/busy text face: busy blocks only — no titles, calendars, locations.
fn format_freebusy(label: &str, events: &[EventInfo]) -> String {
    if events.is_empty() {
        return format!("availability — {label}\n\n  (free)\n");
    }
    let mut out = format!("availability — {label}\n\n");
    for e in events {
        let when = if e.all_day {
            format!("{}  all-day    ", date_of(&e.start))
        } else {
            format!("{}  {}-{}", date_of(&e.start), hhmm(&e.start), hhmm(&e.end))
        };
        out.push_str(&format!("  {when}  busy\n"));
    }
    out
}

/// The Turtle face: the event graph, **skolemized** under `urn:event:{uid}` (the
/// stable iCal UID) so unions and diffs are set operations, never blank-node
/// isomorphism. iCal RDF vocabulary + `ik:calendar` for source provenance.
fn format_turtle(events: &[EventInfo]) -> String {
    let mut ttl = String::from(
        "@prefix ical: <http://www.w3.org/2002/12/cal/ical#> .\n\
         @prefix ik: <https://ikigai-rs.dev/ns#> .\n",
    );
    for e in events {
        let mut props = vec![
            "a ical:Vevent".to_string(),
            format!("ical:uid {}", ttl_str(&e.uid)),
            format!("ical:summary {}", ttl_str(&e.title)),
            format!("ical:dtstart {}", ttl_str(&e.start)),
            format!("ical:dtend {}", ttl_str(&e.end)),
            format!("ik:calendar {}", ttl_str(&e.calendar)),
        ];
        if e.all_day {
            props.push("ik:allDay true".to_string());
        }
        if let Some(location) = &e.location {
            props.push(format!("ical:location {}", ttl_str(location)));
        }
        ttl.push_str(&format!(
            "\n<urn:event:{}> {} .\n",
            e.uid.replace(['<', '>', ' '], "-"),
            props.join(" ;\n    ")
        ));
    }
    ttl
}

/// A Turtle string literal (quote-and-escape).
fn ttl_str(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('\"', "\\\"")
            .replace('\n', " ")
    )
}

/// The events for an invocation: period from the grammar binding (default
/// `week`), optional `calendar=` filter — real EventKit.
fn events_for(inv: &Invocation<'_>) -> Result<Option<(String, Vec<EventInfo>)>> {
    let period = inv
        .bindings
        .get("period")
        .map(str::to_string)
        .unwrap_or_else(|| "week".to_string());
    let (start, end, label) = period_range(&period, Local::now().date_naive())?;
    let calendar = inv.inline_str("calendar").ok().map(str::to_string);
    match platform::events(start, end, calendar.as_deref()) {
        None => Ok(None),
        Some(Ok(events)) => Ok(Some((label, events))),
        Some(Err(why)) => Err(Error::Endpoint(why)),
    }
}

/// `calendar`: real events for a period, projected on the capability — full
/// detail with `…:read:detail` (and a Turtle face via `as=text/turtle`),
/// free/busy with `…:read:freebusy`, denied with neither.
pub fn calendar() -> FnEndpoint {
    FnEndpoint::new("calendar", |inv: &Invocation<'_>| {
        let detail = inv
            .capability
            .allows(&read_scope("urn:personal:calendar", Some("detail")));
        let freebusy = inv
            .capability
            .allows(&read_scope("urn:personal:calendar", Some("freebusy")));
        if !detail && !freebusy {
            return Err(denied(
                "calendar",
                "urn:cap:personal:calendar:read:detail or :freebusy",
            ));
        }
        // The Turtle face carries titles — gate on detail BEFORE touching the
        // platform, so a freebusy holder never even triggers the fetch.
        let want_turtle = inv
            .inline_str("as")
            .map(|s| s.contains("turtle"))
            .unwrap_or(false);
        if want_turtle && !detail {
            return Err(denied(
                "calendar (turtle carries titles)",
                "urn:cap:personal:calendar:read:detail",
            ));
        }
        let Some((label, events)) = events_for(inv)? else {
            return resolve("calendar", None);
        };
        if want_turtle {
            return Ok(Representation::new(
                ReprType::new("text/turtle").with_param("charset", "utf-8"),
                format_turtle(&events).into_bytes(),
            ));
        }
        let body = if detail {
            format_detail(&label, &events)
        } else {
            format_freebusy(&label, &events)
        };
        Ok(Representation::new(text_plain_utf8(), body.into_bytes()))
    })
    .with_description(
        Description::new("calendar")
            .title("Calendar")
            .summary(
                "Real calendar events for a period (urn:personal:calendar:{period}: today, \
                 tomorrow, week, month, a month name, YYYY-MM, YYYY-MM-DD; bare = week). \
                 Projects on the capability: full detail under `…:read:detail` (and a \
                 skolemized Turtle event graph via as=text/turtle), busy blocks only under \
                 `…:read:freebusy`.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("period")
                    .summary("the time window, captured from the IRI (default: week)")
                    .binding(),
            )
            .input(
                ArgSpec::new("calendar")
                    .summary("restrict to one named calendar")
                    .optional(),
            )
            .input(
                ArgSpec::new("as")
                    .summary("text/turtle for the skolemized event graph (detail-gated)")
                    .optional(),
            )
            .output("text/plain;charset=utf-8"),
    )
}

/// `availability`: the free/busy projection for a period — busy blocks only,
/// gated on `urn:cap:personal:calendar:read:freebusy`.
pub fn availability() -> FnEndpoint {
    FnEndpoint::new("availability", |inv: &Invocation<'_>| {
        let scope = read_scope("urn:personal:calendar", Some("freebusy"));
        if !inv.capability.allows(&scope) {
            return Err(denied("availability", &scope));
        }
        let Some((label, events)) = events_for(inv)? else {
            return resolve("availability", None);
        };
        Ok(Representation::new(
            text_plain_utf8(),
            format_freebusy(&label, &events).into_bytes(),
        ))
    })
    .with_description(
        Description::new("availability")
            .title("Availability")
            .summary(
                "Free/busy projection for a period — busy blocks only, no titles or \
                 attendees. The same minimized view `calendar` yields under a free/busy \
                 capability.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("period")
                    .summary("the time window (default: week)")
                    .binding(),
            )
            .output("text/plain;charset=utf-8"),
    )
}

/// Configuration for the consolidated-view calendar machinery — the
/// hand-editable file a host loads (conventionally
/// `~/.config/ikigai/calendar.json`).
///
/// ```json
/// { "view": "Brian-Busy",
///   "account": "iCloud",
///   "sources": ["Brian", "Bosatsu"],
///   "inbox": "Brian-New" }
/// ```
#[derive(Clone, Debug, Deserialize)]
pub struct CalendarConfig {
    /// The derived, sync-owned view calendar the household subscribes to
    /// (e.g. "Brian-Busy"). Never a source; regenerable.
    pub view: String,
    /// Native calendars unioned into the view — an explicit allowlist, never
    /// "all calendars" (the view itself and the inbox live in the same store).
    #[serde(default)]
    pub sources: Vec<String>,
    /// The phone-capture inbox calendar, drained into org (e.g. "Brian-New").
    #[serde(default)]
    pub inbox: Option<String>,
    /// The account (EventKit source) to create calendars on, e.g. "iCloud".
    /// Absent: prefer an iCloud source, else the system default.
    #[serde(default)]
    pub account: Option<String>,
}

impl CalendarConfig {
    /// Parse the hand-editable JSON config.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| {
            Error::Endpoint(format!("urn:personal:calendar:config: invalid JSON: {e}"))
        })
    }
}

/// `calendars`: the native calendar collection. `Source` lists every calendar
/// with its account; `Sink` creates one — `name=` (default: the configured
/// view) on `account=` (default: configured account, else iCloud, else the
/// system default). Real EventKit; the first use may raise the macOS
/// calendar-access prompt.
pub fn calendars(config: Option<CalendarConfig>) -> FnEndpoint {
    FnEndpoint::new("calendars", move |inv: &Invocation<'_>| {
        match inv.request.verb {
            Verb::Sink => {
                let scope = "urn:cap:personal:calendar:write";
                if !inv.capability.allows(scope) {
                    return Err(denied("calendars", scope));
                }
                let name = inv
                    .inline_str("name")
                    .map(str::to_string)
                    .ok()
                    .or_else(|| config.as_ref().map(|c| c.view.clone()))
                    .ok_or_else(|| {
                        Error::Endpoint(
                            "urn:personal:calendars: no name= given and no view configured \
                             (see urn:personal:calendar:config)"
                                .to_string(),
                        )
                    })?;
                let account = inv
                    .inline_str("account")
                    .map(str::to_string)
                    .ok()
                    .or_else(|| config.as_ref().and_then(|c| c.account.clone()));
                let made = platform::create_calendar(&name, account.as_deref());
                resolve("calendars", flatten(made)?)
            }
            _ => {
                let scope = read_scope("urn:personal:calendar", Some("detail"));
                if !inv.capability.allows(&scope) {
                    return Err(denied("calendars", &scope));
                }
                let listed = flatten(platform::calendars().map(|r| {
                    r.map(|calendars| {
                        calendars
                            .iter()
                            .map(|c| format!("{}  [{}]", c.title, c.account))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                }))?;
                resolve("calendars", listed)
            }
        }
    })
    .with_description(
        Description::new("calendars")
            .title("Calendars")
            .summary(
                "The native calendar collection. Source lists every calendar with its \
                 account; Sink creates one (name= — default the configured view — on \
                 account=, default iCloud). Real EventKit; first use may prompt for \
                 calendar access.",
            )
            .verb(Verb::Source)
            .verb(Verb::Sink)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("name")
                    .summary("Sink: the calendar to create (default: the configured view)")
                    .optional(),
            )
            .input(
                ArgSpec::new("account")
                    .summary("Sink: the account to create it on (default: configured, else iCloud)")
                    .optional(),
            )
            .output("text/plain;charset=utf-8"),
    )
}

/// Unwrap the platform Option (None = unsupported OS) and the backend Result
/// (Err = a real failure, e.g. TCC denied) into the endpoint's shape.
fn flatten(outcome: Option<std::result::Result<String, String>>) -> Result<Option<String>> {
    match outcome {
        None => Ok(None),
        Some(Ok(body)) => Ok(Some(body)),
        Some(Err(why)) => Err(Error::Endpoint(why)),
    }
}

/// `calendar:config`: the effective consolidated-view configuration the host
/// loaded (view name, source allowlist, inbox, account) — or a pointer to
/// create the file if none was found.
pub fn calendar_config(config: Option<CalendarConfig>) -> FnEndpoint {
    FnEndpoint::new("calendar-config", move |_inv: &Invocation<'_>| {
        let Some(config) = &config else {
            return Err(Error::Endpoint(
                "no calendar config loaded — create ~/.config/ikigai/calendar.json \
                 with { \"view\": …, \"sources\": [...], \"inbox\": …, \"account\": … }"
                    .to_string(),
            ));
        };
        let body = serde_json::json!({
            "view": config.view,
            "sources": config.sources,
            "inbox": config.inbox,
            "account": config.account,
        });
        Ok(Representation::new(
            ReprType::new("application/json").with_param("charset", "utf-8"),
            serde_json::to_vec(&body).unwrap_or_default(),
        )
        .cacheable())
    })
    .with_description(
        Description::new("calendar-config")
            .title("Calendar view config")
            .summary(
                "The effective consolidated-view configuration: the view calendar's name, \
                 the source allowlist, the capture inbox, and the account.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .output("application/json"),
    )
}

/// The personal-contexts space: binds the resources at `urn:personal:*`. The
/// optional [`CalendarConfig`] (host-loaded, conventionally
/// `~/.config/ikigai/calendar.json`) parameterizes the consolidated-view
/// machinery; `None` leaves the read resources fully functional.
pub fn space(config: Option<CalendarConfig>) -> EndpointSpace {
    EndpointSpace::new()
        .bind(Exact::new("urn:personal:contacts"), contacts())
        .bind(Exact::new("urn:personal:calendar"), calendar())
        .bind(Exact::new("urn:personal:availability"), availability())
        .bind(
            Exact::new("urn:personal:calendars"),
            calendars(config.clone()),
        )
        .bind(
            Exact::new("urn:personal:calendar:config"),
            calendar_config(config),
        )
        // AFTER the exact binds: the period grammar must not shadow
        // urn:personal:calendar:config (first grammar match wins).
        .bind(
            UriTemplate::parse("urn:personal:calendar:{period}").expect("valid template"),
            calendar(),
        )
}

/// Whether a personal backend is implemented for the platform this was built for.
pub fn is_supported() -> bool {
    platform::SUPPORTED
}

/// The human-readable name of the platform this was built for.
pub fn platform_name() -> &'static str {
    platform::NAME
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use ikigai_core::{Capability, Iri, Kernel, Request};
    use std::sync::Arc;

    fn source(iri: &str, capability: &Capability) -> Result<Representation> {
        let kernel = Kernel::new(Arc::new(space(None)));
        block_on(kernel.issue(
            Request::new(Verb::Source, Iri::parse(iri).unwrap()),
            capability,
        ))
    }

    #[test]
    fn calendar_config_parses() {
        let config = CalendarConfig::from_json(
            r#"{ "view": "Brian-Busy", "account": "iCloud",
                 "sources": ["Brian", "Bosatsu"], "inbox": "Brian-New" }"#,
        )
        .unwrap();
        assert_eq!(config.view, "Brian-Busy");
        assert_eq!(config.sources, ["Brian", "Bosatsu"]);
        assert_eq!(config.inbox.as_deref(), Some("Brian-New"));
        assert_eq!(config.account.as_deref(), Some("iCloud"));
        let minimal = CalendarConfig::from_json(r#"{ "view": "V" }"#).unwrap();
        assert!(minimal.sources.is_empty());
    }

    #[test]
    fn creating_a_calendar_requires_the_write_capability() {
        let kernel = Kernel::new(Arc::new(space(None)));
        let read_only =
            Capability::root().attenuate(["urn:cap:personal:calendar:read:detail".to_string()]);
        let denied = block_on(
            kernel.issue(
                Request::new(Verb::Sink, Iri::parse("urn:personal:calendars").unwrap())
                    .with_arg("name", ikigai_core::ArgRef::Inline(b"X".to_vec())),
                &read_only,
            ),
        );
        assert!(format!("{:?}", denied.unwrap_err()).contains("not authorized"));
    }

    #[test]
    fn listing_calendars_requires_detail_read() {
        let kernel = Kernel::new(Arc::new(space(None)));
        let freebusy =
            Capability::root().attenuate(["urn:cap:personal:calendar:read:freebusy".to_string()]);
        let denied = block_on(kernel.issue(
            Request::new(Verb::Source, Iri::parse("urn:personal:calendars").unwrap()),
            &freebusy,
        ));
        assert!(denied.is_err(), "calendar names are detail, not freebusy");
    }

    #[test]
    fn the_config_resource_reports_or_guides() {
        let kernel = Kernel::new(Arc::new(space(Some(
            CalendarConfig::from_json(r#"{ "view": "Brian-Busy", "sources": ["Brian"] }"#).unwrap(),
        ))));
        let out = block_on(kernel.issue(
            Request::new(
                Verb::Source,
                Iri::parse("urn:personal:calendar:config").unwrap(),
            ),
            &Capability::root(),
        ))
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.bytes).unwrap();
        assert_eq!(v["view"], "Brian-Busy");

        let bare = Kernel::new(Arc::new(space(None)));
        let err = block_on(bare.issue(
            Request::new(
                Verb::Source,
                Iri::parse("urn:personal:calendar:config").unwrap(),
            ),
            &Capability::root(),
        ));
        assert!(format!("{:?}", err.unwrap_err()).contains(".config/ikigai/calendar.json"));
    }

    #[test]
    fn an_empty_capability_is_denied_everywhere() {
        let none = Capability::root().attenuate(Vec::<String>::new());
        for iri in [
            "urn:personal:contacts",
            "urn:personal:calendar",
            "urn:personal:availability",
        ] {
            let err = source(iri, &none).unwrap_err();
            assert!(format!("{err:?}").contains("not authorized"), "{iri}");
        }
    }

    fn sample_events() -> Vec<EventInfo> {
        vec![
            EventInfo {
                uid: "ABC-123".into(),
                title: "Design review".into(),
                calendar: "Bosatsu".into(),
                start: "2026-07-02T11:00:00-07:00".into(),
                end: "2026-07-02T12:00:00-07:00".into(),
                all_day: false,
                location: Some("Zoom".into()),
            },
            EventInfo {
                uid: "DEF 456".into(),
                title: "Dinner — \"Ada\"".into(),
                calendar: "Brian".into(),
                start: "2026-07-02T18:30:00-07:00".into(),
                end: "2026-07-02T19:30:00-07:00".into(),
                all_day: false,
                location: None,
            },
        ]
    }

    #[test]
    fn period_ranges_are_correct() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 2).unwrap(); // a Thursday
        let (s, e, _) = period_range("today", today).unwrap();
        assert_eq!((e - s), 86_400);
        let (s, e, label) = period_range("week", today).unwrap();
        assert_eq!((e - s), 7 * 86_400);
        assert!(label.contains("2026-06-29"), "Monday start: {label}");
        let (_, _, label) = period_range("month", today).unwrap();
        assert_eq!(label, "2026-07");
        let (_, _, label) = period_range("june", today).unwrap();
        assert_eq!(label, "2026-06");
        let (_, _, label) = period_range("2025-12", today).unwrap();
        assert_eq!(label, "2025-12");
        let (s, e, _) = period_range("2026-07-15", today).unwrap();
        assert_eq!((e - s), 86_400);
        assert!(period_range("fortnight", today).is_err());
        assert!(period_range("2026-13", today).is_err());
    }

    #[test]
    fn detail_shows_titles_and_freebusy_hides_them() {
        let events = sample_events();
        let detail = format_detail("today", &events);
        assert!(detail.contains("Design review"));
        assert!(detail.contains("[Bosatsu]"));
        assert!(detail.contains("@ Zoom"));
        let busy = format_freebusy("today", &events);
        assert!(busy.contains("busy"));
        assert!(!busy.contains("Design review"));
        assert!(!busy.contains("Bosatsu"));
        assert!(!busy.contains("Zoom"));
    }

    #[test]
    fn turtle_is_skolemized_and_escaped() {
        let ttl = format_turtle(&sample_events());
        assert!(ttl.contains("<urn:event:ABC-123> a ical:Vevent"));
        // the UID with a space is made IRI-safe
        assert!(ttl.contains("<urn:event:DEF-456>"));
        // the quoted title survives escaping
        assert!(ttl.contains("ical:summary \"Dinner — \\\"Ada\\\"\""));
        assert!(ttl.contains("ik:calendar \"Bosatsu\""));
        assert!(!ttl.contains("_:"), "no blank nodes — diff must be set ops");
    }

    #[test]
    fn a_freebusy_capability_is_denied_the_turtle_face() {
        // Turtle carries titles, so it needs detail — the check fires BEFORE any
        // platform/EventKit call, so this is platform-neutral.
        let kernel = Kernel::new(Arc::new(space(None)));
        let freebusy =
            Capability::root().attenuate(["urn:cap:personal:calendar:read:freebusy".to_string()]);
        let denied = block_on(
            kernel.issue(
                Request::new(Verb::Source, Iri::parse("urn:personal:calendar").unwrap())
                    .with_arg("as", ikigai_core::ArgRef::Inline(b"text/turtle".to_vec())),
                &freebusy,
            ),
        );
        let msg = format!("{:?}", denied.unwrap_err());
        assert!(msg.contains("read:detail"), "{msg}");
    }

    #[test]
    fn an_unknown_period_is_rejected_at_the_grammar() {
        let kernel = Kernel::new(Arc::new(space(None)));
        let denied = block_on(kernel.issue(
            Request::new(
                Verb::Source,
                Iri::parse("urn:personal:calendar:fortnight").unwrap(),
            ),
            &Capability::root(),
        ));
        assert!(format!("{:?}", denied.unwrap_err()).contains("unknown period"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn authorized_but_unsupported_platform_reports_unsupported() {
        // Root passes the capability check, then the platform has no backend.
        let err = source("urn:personal:contacts", &Capability::root()).unwrap_err();
        assert!(format!("{err:?}").contains("not supported"));
        assert!(!is_supported());
    }
}
