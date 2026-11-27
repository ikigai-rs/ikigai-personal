//! A calendar off the machine: the teacher's own calendar reached over the
//! network, for a host without EventKit.
//!
//! Two sources, both configured by SECRET NAME, never by value:
//!
//! - **A free/busy address**: the published iCalendar URL of the calendar the
//!   person already uses (Google's secret iCal address, an iCloud public calendar,
//!   a Fastmail free/busy link). Read-only, no OAuth. Read with `urn:http:get`.
//! - **CalDAV**: one calendar collection, read with a `calendar-query` REPORT and
//!   written by uid (`{collection}/{uid}.ics`), with Basic credentials.
//!
//! Every byte crosses the kernel: the URL and credentials come from
//! `urn:secret:{name}` and the requests go through `urn:http:*`, both under the
//! CALLER's capability, so `urn:cap:secret:read:<name>` and `urn:cap:net:<host>`
//! decide what reaches the network, and nothing here opens a socket.

use crate::caldav::{self, DavRequest};
use crate::platform::EventInfo;
use crate::store::{EventWrite, PutOutcome};
use ikigai_core::{ArgRef, Error, Invocation, Iri, Request, Result, Verb};
use serde::{Deserialize, Serialize};

/// The remote calendar a host reads busy time from and writes sessions into.
/// Every value is the NAME of a secret in `urn:secret:*`; the config file holds
/// no URL and no credential.
///
/// ```json
/// { "remote": {
///     "freebusy": ["teacher-freebusy-url"],
///     "caldav": { "calendar": "Teaching", "url": "teaching-caldav-url",
///                 "user": "teaching-caldav-user", "password": "teaching-caldav-password" } } }
/// ```
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct RemoteConfig {
    /// Secrets that each hold one free/busy iCalendar URL (`https://` or
    /// `webcal://`). Their busy blocks are unioned.
    #[serde(default)]
    pub freebusy: Vec<String>,
    /// One CalDAV calendar: read for busy blocks, and the target of writes that
    /// name it.
    #[serde(default)]
    pub caldav: Option<CaldavConfig>,
}

/// One CalDAV calendar collection.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CaldavConfig {
    /// The calendar name a write's `calendar=` must match to reach this
    /// collection (the person's existing calendar, e.g. "Teaching").
    pub calendar: String,
    /// Secret holding the collection URL.
    pub url: String,
    /// Secret holding the user name.
    pub user: String,
    /// Secret holding the password (an app-specific password).
    pub password: String,
}

impl RemoteConfig {
    /// Whether anything is configured.
    pub fn is_empty(&self) -> bool {
        self.freebusy.is_empty() && self.caldav.is_none()
    }
}

/// A secret's text, trailing whitespace (a stored newline) removed.
async fn secret(inv: &Invocation<'_>, name: &str) -> Result<String> {
    let iri = Iri::parse(format!("urn:secret:{name}")).map_err(|_| Error::InvalidArgument {
        name: "remote".to_string(),
        detail: format!("the secret name {name:?} does not form an IRI"),
    })?;
    let repr = inv.issue(Request::new(Verb::Source, iri)).await?;
    String::from_utf8(repr.bytes)
        .map(|text| text.trim_end().to_string())
        .map_err(|_| Error::Endpoint(format!("the secret {name} is not UTF-8 text")))
}

fn http_iri(method: &str) -> Iri {
    Iri::parse(format!("urn:http:{method}")).expect("valid IRI")
}

fn inline(request: Request, name: &str, value: &str) -> Request {
    request.with_arg(name, ArgRef::Inline(value.as_bytes().to_vec()))
}

/// `webcal://` is how calendar apps advertise a subscription; it is `https://`
/// on the wire, and `urn:http` sends only http and https.
fn https_url(url: &str) -> String {
    match url.strip_prefix("webcal://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_string(),
    }
}

/// Busy time, as events, from every configured source over `[start, end)`.
/// `unexpanded` counts feed events whose later occurrences are missing.
pub struct Busy {
    pub events: Vec<EventInfo>,
    pub unexpanded: usize,
}

/// Read every configured source. A source that fails fails the read: a missing
/// source would read as free time, which is the one wrong answer availability
/// must never give.
pub async fn busy(
    inv: &Invocation<'_>,
    config: &RemoteConfig,
    start: i64,
    end: i64,
) -> Result<Busy> {
    let mut events = Vec::new();
    let mut unexpanded = 0;
    for name in &config.freebusy {
        let url = https_url(&secret(inv, name).await?);
        let request = inline(
            inline(Request::new(Verb::Source, http_iri("get")), "url", &url),
            "accept",
            "text/calendar",
        );
        let body = inv.issue(request).await?;
        let feed = caldav::parse_feed(&String::from_utf8_lossy(&body.bytes), "free/busy");
        unexpanded += feed.unexpanded;
        events.extend(feed.events);
    }
    if let Some(dav) = &config.caldav {
        let session = Caldav::open(inv, dav).await?;
        let query = caldav::report_calendar_query(&session.collection, start, end);
        let body = session.send(inv, "report", Verb::Sink, &query).await?;
        events.extend(caldav::parse_events(
            &String::from_utf8_lossy(&body.bytes),
            &dav.calendar,
        ));
    }
    events.retain(|e| overlaps(e, start, end));
    events.sort_by(|a, b| a.start.cmp(&b.start));
    events.dedup_by(|a, b| a.start == b.start && a.end == b.end && a.busy == b.busy);
    Ok(Busy { events, unexpanded })
}

fn epoch(rfc3339: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|t| t.timestamp())
}

/// Whether an event touches `[start, end)`. An event whose times do not parse is
/// kept: dropping it could read as free.
fn overlaps(e: &EventInfo, start: i64, end: i64) -> bool {
    match (epoch(&e.start), epoch(&e.end)) {
        (Some(s), Some(t)) => s < end && (t > start || (t == s && s >= start)),
        _ => true,
    }
}

/// An opened CalDAV collection: its URL and the Authorization value, read from
/// their secrets for one request's lifetime.
struct Caldav {
    collection: String,
    authorization: String,
}

impl Caldav {
    async fn open(inv: &Invocation<'_>, config: &CaldavConfig) -> Result<Self> {
        let collection = secret(inv, &config.url).await?;
        let user = secret(inv, &config.user).await?;
        let password = secret(inv, &config.password).await?;
        let authorization = caldav::basic_auth(&user, &password).map_err(Error::Endpoint)?;
        Ok(Caldav {
            collection,
            authorization,
        })
    }

    /// Carry one sans-IO request over `urn:http:{method}`.
    async fn send(
        &self,
        inv: &Invocation<'_>,
        method: &str,
        verb: Verb,
        dav: &DavRequest,
    ) -> Result<ikigai_core::Representation> {
        let mut request = inline(
            inline(Request::new(verb, http_iri(method)), "url", &dav.path),
            "authorization",
            &self.authorization,
        );
        let mut headers = Vec::new();
        for (name, value) in &dav.headers {
            if name.eq_ignore_ascii_case("Content-Type") {
                request = inline(request, "content_type", value);
            } else {
                headers.push(format!("{name}: {value}"));
            }
        }
        if !headers.is_empty() {
            request = inline(request, "headers", &headers.join("\n"));
        }
        if !dav.body.is_empty() {
            request = inline(request, "content", &dav.body);
        }
        inv.issue(request).await
    }

    fn event_url(&self, uid: &str) -> String {
        caldav::event_path(&self.collection, uid)
    }
}

/// Put one event into the CalDAV calendar by uid. The event lives at
/// `{collection}/{uid}.ics`, so the uid names its resource: a GET says whether it
/// exists (and its entity tag and current times), then a PUT makes it
/// (`If-None-Match: *`) or replaces that version (`If-Match`). A concurrent change
/// on the server is a Conflict, never a silent overwrite.
pub async fn put(
    inv: &Invocation<'_>,
    config: &CaldavConfig,
    uid: &str,
    event: &EventWrite<'_>,
) -> Result<PutOutcome> {
    let session = Caldav::open(inv, config).await?;
    let url = session.event_url(uid);
    let get = DavRequest {
        method: "GET",
        path: url.clone(),
        headers: vec![("Accept".into(), "text/calendar".into())],
        body: String::new(),
    };
    let current = match session.send(inv, "get", Verb::Source, &get).await {
        Ok(repr) => Some(repr),
        Err(Error::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    match current {
        None => {
            let request = caldav::put_event(
                &session.collection,
                uid,
                event.title,
                event.start,
                event.end,
                event.all_day,
                event.location,
                event.description,
                event.alerts,
            );
            session.send(inv, "put", Verb::Sink, &request).await?;
            Ok(PutOutcome::Created)
        }
        Some(repr) => {
            let etag = repr.origin_validator().and_then(|v| v.etag.clone());
            let stored = caldav::parse_ics(&String::from_utf8_lossy(&repr.bytes), &config.calendar);
            let moved = stored.first().is_none_or(|e| {
                epoch(&e.start) != Some(event.start) || epoch(&e.end) != Some(event.end)
            });
            let request = caldav::replace_event(
                &session.collection,
                uid,
                etag.as_deref(),
                event.title,
                event.start,
                event.end,
                event.all_day,
                event.location,
                event.description,
                event.alerts,
            );
            session.send(inv, "put", Verb::Sink, &request).await?;
            Ok(PutOutcome::Updated { moved })
        }
    }
}

/// Delete the event with `uid` from the CalDAV calendar.
pub async fn delete(inv: &Invocation<'_>, config: &CaldavConfig, uid: &str) -> Result<String> {
    let session = Caldav::open(inv, config).await?;
    let request = caldav::delete_event(&session.collection, uid);
    match session.send(inv, "delete", Verb::Delete, &request).await {
        Ok(_) => Ok(format!(
            "removed the event with uid {uid} from {}",
            config.calendar
        )),
        Err(Error::NotFound(_)) => Err(Error::NotFound(format!(
            "no event with uid {uid} in {}",
            config.calendar
        ))),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    //! The remote calendar end to end through a real kernel: the personal space,
    //! ikigai-http's real `urn:http:*` endpoints over an in-memory server, and a
    //! stand-in `urn:secret:{name}`. No socket and no keystore.
    use super::*;
    use crate::{space, CalendarConfig};
    use async_trait::async_trait;
    use futures::executor::block_on;
    use ikigai_core::{
        Capability, EndpointSpace, Fallback, FnEndpoint, Kernel, ReprType, Representation, Space,
        UriTemplate,
    };
    use ikigai_http::{HttpRequest, HttpResponse, HttpTransport};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    const COLLECTION: &str = "https://dav.example.test/calendars/teacher/teaching/";
    const GOOGLE: &str =
        "https://calendar.example.test/calendar/ical/teacher%40example.test/private-0f1e2d/basic.ics";
    const ICLOUD: &str = "webcal://published.example.test/published/2/MTIzNDU2";
    const FASTMAIL: &str = "https://freebusy.example.test/fb/teacher/0f1e2d.ics";

    /// Google's secret iCal address answers a full VCALENDAR export: IANA TZID
    /// times beside a VTIMEZONE, a recurring master (RRULE, not expanded), a
    /// transparent event and an all-day event.
    const GOOGLE_FEED: &str = concat!(
        "BEGIN:VCALENDAR\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\nVERSION:2.0\r\n",
        "X-WR-TIMEZONE:America/Los_Angeles\r\n",
        "BEGIN:VTIMEZONE\r\nTZID:America/Los_Angeles\r\nBEGIN:STANDARD\r\nTZOFFSETFROM:-0700\r\n",
        "TZOFFSETTO:-0800\r\nTZNAME:PST\r\nDTSTART:19701101T020000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\nDTSTART;TZID=America/Los_Angeles:20261202T100000\r\n",
        "DTEND;TZID=America/Los_Angeles:20261202T110000\r\nUID:one-off@google.com\r\n",
        "SUMMARY:Dentist\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nDTSTART;TZID=America/Los_Angeles:20261201T090000\r\n",
        "DTEND;TZID=America/Los_Angeles:20261201T093000\r\nRRULE:FREQ=WEEKLY;BYDAY=TU\r\n",
        "UID:standup@google.com\r\nSUMMARY:Standup\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nDTSTART:20261203T200000Z\r\nDTEND:20261203T210000Z\r\nTRANSP:TRANSPARENT\r\n",
        "UID:reminder@google.com\r\nSUMMARY:Reminder\r\nEND:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );

    /// An iCloud public calendar: VEVENTs in a New York TZID, Apple properties.
    const ICLOUD_FEED: &str = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Apple Inc.//macOS 15.0//EN\r\n",
        "X-WR-CALNAME:Home\r\nBEGIN:VEVENT\r\nUID:6B29FC40-CA47-1067-B31D-00DD010662DA\r\n",
        "DTSTART;TZID=America/New_York:20261204T130000\r\n",
        "DTEND;TZID=America/New_York:20261204T143000\r\nSUMMARY:Lunch\r\n",
        "X-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    );

    /// A VFREEBUSY answer (RFC 5545 section 3.6.4), what a free/busy link such as
    /// Fastmail's serves: UTC periods, by end and by duration, with a FREE period.
    const FREEBUSY_FEED: &str = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//FreeBusy//EN\r\n",
        "BEGIN:VFREEBUSY\r\nDTSTART:20261201T000000Z\r\nDTEND:20261208T000000Z\r\n",
        "FREEBUSY;FBTYPE=BUSY:20261205T150000Z/20261205T160000Z,20261206T150000Z/PT45M\r\n",
        "FREEBUSY;FBTYPE=FREE:20261205T170000Z/20261205T180000Z\r\n",
        "FREEBUSY:20261207T090000Z/P1DT1H\r\n",
        "END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
    );

    /// One request as the server saw it: method, url, headers, body.
    type Logged = (String, String, Vec<(String, String)>, String);

    #[derive(Default)]
    struct Server {
        /// The CalDAV collection's members: url -> (etag, iCalendar body).
        members: Mutex<HashMap<String, (String, String)>>,
        /// Fixed GET answers (the feeds).
        feeds: HashMap<String, String>,
        /// Every request that reached the server: method, url, headers, body.
        log: Mutex<Vec<Logged>>,
        etags: Mutex<u32>,
        /// When set, the next GET of a member is followed by someone else's edit.
        edit_after_get: Mutex<bool>,
    }

    fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn answer(status: u16, headers: Vec<(&str, String)>, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: headers
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            body: body.as_bytes().to_vec().into(),
        }
    }

    #[async_trait]
    impl HttpTransport for Server {
        async fn send(&self, request: HttpRequest) -> std::result::Result<HttpResponse, String> {
            let method = request.method.as_str().to_string();
            let body = String::from_utf8_lossy(&request.body).to_string();
            self.log.lock().unwrap().push((
                method.clone(),
                request.url.clone(),
                request.headers.clone(),
                body.clone(),
            ));
            if let Some(feed) = self.feeds.get(&request.url) {
                return Ok(answer(
                    200,
                    vec![("content-type", "text/calendar".into())],
                    feed,
                ));
            }
            if header(&request.headers, "authorization")
                != Some("Basic dGVhY2hlcjphcHAtcGFzc3dvcmQ=")
            {
                return Ok(answer(401, vec![], ""));
            }
            let mut members = self.members.lock().unwrap();
            Ok(match method.as_str() {
                "GET" => match members.get_mut(&request.url) {
                    Some((etag, ics)) => {
                        let read = answer(
                            200,
                            vec![
                                ("content-type", "text/calendar".into()),
                                ("etag", etag.clone()),
                            ],
                            ics,
                        );
                        if std::mem::take(&mut *self.edit_after_get.lock().unwrap()) {
                            *etag = "\"theirs\"".into();
                        }
                        read
                    }
                    None => answer(404, vec![], ""),
                },
                "PUT" => {
                    let exists = members.get(&request.url).map(|(etag, _)| etag.clone());
                    let refused = match (
                        &exists,
                        header(&request.headers, "if-none-match"),
                        header(&request.headers, "if-match"),
                    ) {
                        (Some(_), Some("*"), _) => true,
                        (Some(etag), _, Some(wanted)) => etag != wanted,
                        (None, _, Some(_)) => true,
                        _ => false,
                    };
                    if refused {
                        answer(412, vec![], "")
                    } else {
                        let mut n = self.etags.lock().unwrap();
                        *n += 1;
                        members.insert(request.url.clone(), (format!("\"v{n}\""), body));
                        answer(if exists.is_some() { 204 } else { 201 }, vec![], "")
                    }
                }
                "DELETE" => match members.remove(&request.url) {
                    Some(_) => answer(204, vec![], ""),
                    None => answer(404, vec![], ""),
                },
                "REPORT" => {
                    let mut xml = String::from(
                        r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">"#,
                    );
                    for (url, (etag, ics)) in members.iter() {
                        xml.push_str(&format!(
                            "<d:response><d:href>{url}</d:href><d:propstat><d:prop><d:getetag>{etag}</d:getetag><c:calendar-data><![CDATA[{ics}]]></c:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
                        ));
                    }
                    xml.push_str("</d:multistatus>");
                    answer(207, vec![("content-type", "application/xml".into())], &xml)
                }
                _ => answer(405, vec![], ""),
            })
        }
    }

    fn secrets(values: &[(&str, &str)]) -> EndpointSpace {
        let values: HashMap<String, String> = values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        EndpointSpace::new().bind(
            UriTemplate::parse("urn:secret:{name}").unwrap(),
            FnEndpoint::new("secret", move |inv: &ikigai_core::Invocation<'_>| {
                let name = inv.bindings.get("name").unwrap_or_default();
                if !inv
                    .capability
                    .allows(&format!("urn:cap:secret:read:{name}"))
                {
                    return Err(Error::Endpoint(format!("no grant for secret {name}")));
                }
                values
                    .get(name)
                    .map(|v| {
                        Representation::new(
                            ReprType::new("text/plain"),
                            format!("{v}\n").into_bytes(),
                        )
                    })
                    .ok_or_else(|| Error::NotFound(format!("no secret {name}")))
            }),
        )
    }

    fn config(remote: RemoteConfig) -> CalendarConfig {
        CalendarConfig {
            view: String::new(),
            sources: Vec::new(),
            inbox: None,
            account: None,
            remote: Some(remote),
        }
    }

    fn caldav_config() -> CaldavConfig {
        CaldavConfig {
            calendar: "Teaching".into(),
            url: "teaching-url".into(),
            user: "teaching-user".into(),
            password: "teaching-password".into(),
        }
    }

    struct Fixture {
        kernel: Kernel,
        server: Arc<Server>,
    }

    fn fixture(remote: RemoteConfig) -> Fixture {
        let server = Arc::new(Server {
            feeds: [
                (GOOGLE.to_string(), GOOGLE_FEED.to_string()),
                (
                    ICLOUD.replace("webcal://", "https://"),
                    ICLOUD_FEED.to_string(),
                ),
                (FASTMAIL.to_string(), FREEBUSY_FEED.to_string()),
            ]
            .into_iter()
            .collect(),
            ..Server::default()
        });
        let spaces: Vec<Arc<dyn Space>> = vec![
            Arc::new(space(Some(config(remote)))),
            Arc::new(ikigai_http::space(server.clone())),
            Arc::new(secrets(&[
                ("google-feed", GOOGLE),
                ("icloud-feed", ICLOUD),
                ("fastmail-feed", FASTMAIL),
                ("teaching-url", COLLECTION),
                ("teaching-user", "teacher"),
                ("teaching-password", "app-password"),
            ])),
        ];
        Fixture {
            kernel: Kernel::new(Arc::new(Fallback::new(spaces))),
            server,
        }
    }

    impl Fixture {
        fn issue(&self, verb: Verb, iri: &str, args: &[(&str, &str)]) -> Result<String> {
            self.issue_as(verb, iri, args, &Capability::root())
        }

        fn issue_as(
            &self,
            verb: Verb,
            iri: &str,
            args: &[(&str, &str)],
            capability: &Capability,
        ) -> Result<String> {
            let mut request = Request::new(verb, Iri::parse(iri).unwrap());
            for (name, value) in args {
                request = inline(request, name, value);
            }
            block_on(self.kernel.issue(request, capability))
                .map(|r| String::from_utf8(r.bytes).unwrap())
        }
    }

    /// The local `HH:MM-HH:MM` a UTC interval renders as on this machine.
    fn local_span(start: &str, end: &str) -> String {
        let at = |v: &str| {
            chrono::DateTime::parse_from_rfc3339(v)
                .unwrap()
                .with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        };
        format!("{}-{}", at(start), at(end))
    }

    const WEEK: &str = "urn:personal:availability:2026-11-30..2026-12-09";

    #[test]
    fn a_google_secret_address_gives_busy_blocks_in_the_right_zone() {
        let f = fixture(RemoteConfig {
            freebusy: vec!["google-feed".into()],
            caldav: None,
        });
        let out = f.issue(Verb::Source, WEEK, &[]).unwrap();
        // 10:00-11:00 Los Angeles is 18:00-19:00 UTC, whatever zone this host runs in.
        let dentist = local_span("2026-12-02T18:00:00Z", "2026-12-02T19:00:00Z");
        assert!(out.contains(&format!("{dentist}  busy")), "{out}");
        assert!(
            !out.contains("Dentist"),
            "titles never reach the free/busy face: {out}"
        );
        let reminder = local_span("2026-12-03T20:00:00Z", "2026-12-03T21:00:00Z");
        assert!(
            !out.contains(&reminder),
            "a transparent event is free: {out}"
        );
        assert!(
            out.contains("note: 1 recurring event(s) in the free/busy feed are not expanded"),
            "an unexpanded RRULE is said, never silently free: {out}"
        );
        let log = f.server.log.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!((log[0].0.as_str(), log[0].1.as_str()), ("GET", GOOGLE));
        assert_eq!(header(&log[0].2, "accept"), Some("text/calendar"));
    }

    #[test]
    fn an_icloud_webcal_address_is_fetched_over_https() {
        let f = fixture(RemoteConfig {
            freebusy: vec!["icloud-feed".into()],
            caldav: None,
        });
        let out = f.issue(Verb::Source, WEEK, &[]).unwrap();
        // 13:00-14:30 New York is 18:00-19:30 UTC.
        let lunch = local_span("2026-12-04T18:00:00Z", "2026-12-04T19:30:00Z");
        assert!(out.contains(&format!("{lunch}  busy")), "{out}");
        assert!(!out.contains("note:"), "{out}");
        assert_eq!(
            f.server.log.lock().unwrap()[0].1,
            "https://published.example.test/published/2/MTIzNDU2"
        );
    }

    #[test]
    fn a_vfreebusy_link_gives_its_busy_periods_and_drops_free_ones() {
        let f = fixture(RemoteConfig {
            freebusy: vec!["fastmail-feed".into()],
            caldav: None,
        });
        let out = f.issue(Verb::Source, WEEK, &[]).unwrap();
        for (start, end) in [
            ("2026-12-05T15:00:00Z", "2026-12-05T16:00:00Z"),
            ("2026-12-06T15:00:00Z", "2026-12-06T15:45:00Z"),
        ] {
            let span = local_span(start, end);
            assert!(out.contains(&format!("{span}  busy")), "{span}: {out}");
        }
        let free = local_span("2026-12-05T17:00:00Z", "2026-12-05T18:00:00Z");
        assert!(!out.contains(&free), "FBTYPE=FREE is not busy: {out}");
        // P1DT1H from 12-07 09:00Z runs to 12-08 10:00Z: both days are blocked.
        let feed = crate::caldav::parse_feed(FREEBUSY_FEED, "fb");
        let long = feed.events.last().unwrap();
        assert_eq!(
            epoch(&long.end).unwrap() - epoch(&long.start).unwrap(),
            25 * 3_600
        );
    }

    #[test]
    fn several_feeds_are_unioned() {
        let f = fixture(RemoteConfig {
            freebusy: vec!["google-feed".into(), "icloud-feed".into()],
            caldav: None,
        });
        let out = f.issue(Verb::Source, WEEK, &[]).unwrap();
        assert!(out.contains(&local_span("2026-12-02T18:00:00Z", "2026-12-02T19:00:00Z")));
        assert!(out.contains(&local_span("2026-12-04T18:00:00Z", "2026-12-04T19:30:00Z")));
    }

    #[test]
    fn a_source_that_fails_fails_the_read_instead_of_reading_free() {
        let f = fixture(RemoteConfig {
            freebusy: vec!["google-feed".into(), "missing-secret".into()],
            caldav: None,
        });
        let err = f.issue(Verb::Source, WEEK, &[]).unwrap_err();
        assert!(format!("{err:?}").contains("missing-secret"), "{err:?}");
    }

    #[test]
    fn a_put_by_uid_creates_then_moves_the_caldav_event_in_place() {
        let f = fixture(RemoteConfig {
            freebusy: Vec::new(),
            caldav: Some(caldav_config()),
        });
        let iri = "urn:personal:calendar:event:uid:class-7%2Fsession-3";
        let first = f
            .issue(
                Verb::Sink,
                iri,
                &[
                    ("calendar", "Teaching"),
                    ("title", "Session 3"),
                    ("start", "2026-12-03T18:00:00Z"),
                    ("alert", "15m"),
                ],
            )
            .unwrap();
        assert_eq!(first, "created \"Session 3\" in Teaching");
        let moved = f
            .issue(
                Verb::Sink,
                iri,
                &[
                    ("calendar", "Teaching"),
                    ("title", "Session 3"),
                    ("start", "2026-12-10T18:00:00Z"),
                ],
            )
            .unwrap();
        assert_eq!(
            moved,
            "updated \"Session 3\" in Teaching: moved to its new time"
        );

        let url = format!("{COLLECTION}class-7%2Fsession-3.ics");
        let members = f.server.members.lock().unwrap();
        assert_eq!(members.len(), 1, "one entry, moved, never a second copy");
        let (_, ics) = &members[&url];
        let stored = crate::caldav::parse_ics(ics, "Teaching");
        assert_eq!(stored[0].uid, "class-7/session-3");
        assert_eq!(epoch(&stored[0].start), epoch("2026-12-10T18:00:00Z"));
        assert!(
            stored[0].alerts.is_empty(),
            "the put stated no alert, so none is kept"
        );
        drop(members);

        let log = f.server.log.lock().unwrap();
        let shape: Vec<(&str, Option<&str>, Option<&str>)> = log
            .iter()
            .map(|(m, _, h, _)| {
                (
                    m.as_str(),
                    header(h, "if-none-match"),
                    header(h, "if-match"),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("GET", None, None),
                ("PUT", Some("*"), None),
                ("GET", None, None),
                ("PUT", None, Some("\"v1\"")),
            ],
            "create guards against an existing event; the update replaces only the version read"
        );
        assert!(log.iter().all(|(_, u, _, _)| u == &url));
    }

    #[test]
    fn an_unchanged_put_is_an_update_that_did_not_move() {
        let f = fixture(RemoteConfig {
            freebusy: Vec::new(),
            caldav: Some(caldav_config()),
        });
        let args = [
            ("calendar", "Teaching"),
            ("title", "Office hours"),
            ("start", "2026-12-03T18:00:00Z"),
        ];
        let iri = "urn:personal:calendar:event:uid:hours-1";
        f.issue(Verb::Sink, iri, &args).unwrap();
        let again = f.issue(Verb::Sink, iri, &args).unwrap();
        assert_eq!(again, "updated \"Office hours\" in Teaching");
    }

    #[test]
    fn caldav_busy_blocks_and_delete_by_uid() {
        let f = fixture(RemoteConfig {
            freebusy: Vec::new(),
            caldav: Some(caldav_config()),
        });
        let iri = "urn:personal:calendar:event:uid:session-4";
        f.issue(
            Verb::Sink,
            iri,
            &[
                ("calendar", "Teaching"),
                ("title", "Session 4"),
                ("start", "2026-12-04T18:00:00Z"),
            ],
        )
        .unwrap();
        let out = f.issue(Verb::Source, WEEK, &[]).unwrap();
        let span = local_span("2026-12-04T18:00:00Z", "2026-12-04T19:00:00Z");
        assert!(out.contains(&format!("{span}  busy")), "{out}");
        assert!(!out.contains("Session 4"), "{out}");
        {
            let log = f.server.log.lock().unwrap();
            let report = log.iter().find(|(m, ..)| m == "REPORT").unwrap();
            assert_eq!(report.1, COLLECTION);
            assert_eq!(header(&report.2, "depth"), Some("1"));
            assert!(report.3.contains("calendar-query") && report.3.contains("expand"));
        }

        let removed = f
            .issue(Verb::Delete, iri, &[("calendar", "Teaching")])
            .unwrap();
        assert_eq!(
            removed,
            "removed the event with uid session-4 from Teaching"
        );
        let again = f
            .issue(Verb::Delete, iri, &[("calendar", "Teaching")])
            .unwrap_err();
        assert!(matches!(again, Error::NotFound(_)), "{again:?}");
    }

    #[test]
    fn a_concurrent_change_on_the_server_is_a_conflict() {
        let f = fixture(RemoteConfig {
            freebusy: Vec::new(),
            caldav: Some(caldav_config()),
        });
        let iri = "urn:personal:calendar:event:uid:session-5";
        let args = [
            ("calendar", "Teaching"),
            ("title", "Session 5"),
            ("start", "2026-12-04T18:00:00Z"),
        ];
        f.issue(Verb::Sink, iri, &args).unwrap();
        // Someone else edits the event between our GET and our PUT.
        *f.server.edit_after_get.lock().unwrap() = true;
        let err = f.issue(Verb::Sink, iri, &args).unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "{err:?}");
        let url = format!("{COLLECTION}session-5.ics");
        assert_eq!(
            f.server.members.lock().unwrap()[&url].0,
            "\"theirs\"",
            "their edit stands; ours was not written over it"
        );
    }

    #[test]
    fn a_host_without_a_platform_store_names_its_caldav_calendar() {
        if crate::platform::SUPPORTED {
            return; // On a Mac another calendar name is EventKit's, not an error.
        }
        let f = fixture(RemoteConfig {
            freebusy: Vec::new(),
            caldav: Some(caldav_config()),
        });
        let err = f
            .issue(
                Verb::Sink,
                "urn:personal:calendar:event:uid:s",
                &[
                    ("calendar", "Home"),
                    ("title", "T"),
                    ("start", "2026-12-04"),
                ],
            )
            .unwrap_err();
        assert!(
            matches!(err, Error::InvalidArgument { ref name, ref detail } if name == "calendar" && detail.contains("\"Teaching\"")),
            "{err:?}"
        );
        assert!(f.server.log.lock().unwrap().is_empty());
    }

    #[test]
    fn the_remote_reads_and_writes_declare_what_they_need() {
        let f = fixture(RemoteConfig {
            freebusy: vec!["google-feed".into()],
            caldav: Some(caldav_config()),
        });
        // Free/busy alone cannot read the feed's secret: refused before any request.
        let freebusy = Capability::scoped(["urn:cap:personal:calendar:read:freebusy"]);
        let err = f.issue_as(Verb::Source, WEEK, &[], &freebusy).unwrap_err();
        assert!(matches!(err, Error::Denied(_)), "{err:?}");
        assert!(f.server.log.lock().unwrap().is_empty());
        // The write needs the cut authority urn:http's PUT requires, beside the rest.
        let writer = Capability::scoped([
            "urn:cap:personal:calendar:write",
            "urn:cap:secret:read:teaching-url",
            "urn:cap:net:dav.example.test",
        ]);
        let err = f
            .issue_as(
                Verb::Sink,
                "urn:personal:calendar:event:uid:s",
                &[
                    ("calendar", "Teaching"),
                    ("title", "T"),
                    ("start", "2026-12-04"),
                ],
                &writer,
            )
            .unwrap_err();
        assert!(matches!(err, Error::Denied(_)), "{err:?}");
        assert!(f.server.log.lock().unwrap().is_empty());
    }

    #[test]
    fn narrow_grants_are_enough_to_read_a_feed() {
        let f = fixture(RemoteConfig {
            freebusy: vec!["google-feed".into()],
            caldav: None,
        });
        let narrow = Capability::scoped([
            "urn:cap:personal:calendar:read:freebusy",
            "urn:cap:secret:read:google-feed",
            "urn:cap:net:calendar.example.test",
        ]);
        let out = f.issue_as(Verb::Source, WEEK, &[], &narrow).unwrap();
        assert!(out.contains("busy"), "{out}");
    }

    #[test]
    fn the_config_reports_secret_names_never_values() {
        let f = fixture(RemoteConfig {
            freebusy: vec!["google-feed".into()],
            caldav: Some(caldav_config()),
        });
        let out = f
            .issue(Verb::Source, "urn:personal:calendar:config", &[])
            .unwrap();
        assert!(
            out.contains("google-feed") && out.contains("teaching-password"),
            "{out}"
        );
        assert!(
            !out.contains("app-password") && !out.contains("private-0f1e2d"),
            "{out}"
        );
    }

    #[test]
    fn remote_config_parses_from_the_calendar_file() {
        let config = CalendarConfig::from_json(
            r#"{ "remote": { "freebusy": ["google-feed"],
                 "caldav": { "calendar": "Teaching", "url": "u", "user": "n", "password": "p" } } }"#,
        )
        .unwrap();
        assert_eq!(config.view, "", "a remote-only host needs no view");
        let remote = config.remote.unwrap();
        assert_eq!(remote.freebusy, ["google-feed"]);
        assert_eq!(remote.caldav.unwrap().calendar, "Teaching");
        // A file an older release wrote still parses, with no remote.
        let older = CalendarConfig::from_json(r#"{ "view": "V", "sources": ["A"] }"#).unwrap();
        assert!(older.remote.is_none());
    }
}
