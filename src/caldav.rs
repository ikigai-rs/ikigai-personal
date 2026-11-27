//! Sans-IO CalDAV core: builds the WebDAV requests a calendar backend needs and
//! parses the answers into [`CalendarInfo`] / [`EventInfo`]. No socket, no HTTP
//! client — the caller carries a [`DavRequest`] over the kernel's `urn:http`
//! (so `urn:cap:net:<host>` gates and records every call) and hands the body
//! back to a parser here.
//!
//! It also reads a published iCalendar feed (a free/busy address): VEVENTs and
//! VFREEBUSY periods, through [`parse_feed`].
//!
//! Limits: a `TZID` that names an IANA zone (`America/Los_Angeles`) is resolved
//! with the tz database; any other `TZID` (a Windows zone name) is read as local
//! floating time. CalDAV recurrence expansion is delegated to the server
//! (`<c:expand>`); a published feed is NOT expanded here, and [`parse_feed`] counts
//! the recurring events it could not expand so the caller can say so.
//! vCard/CardDAV is not covered. `crate::remote` carries these requests over
//! `urn:http`; tested against canned server answers, not yet against a live
//! iCloud/Fastmail/Nextcloud/Radicale server.

use crate::platform::{occurrence_uid, CalendarInfo, EventInfo};
use chrono::{Days, Duration, Local, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

/// RFC 3986 unreserved characters stay; everything else in a uid is escaped, so a
/// uid is always exactly one path segment.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// One WebDAV request, ready for `urn:http`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DavRequest {
    pub method: &'static str,
    /// Absolute path (or URL) on the server.
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

fn xml_headers(depth: &str) -> Vec<(String, String)> {
    vec![
        (
            "Content-Type".into(),
            "application/xml; charset=utf-8".into(),
        ),
        ("Depth".into(), depth.into()),
    ]
}

/// `Authorization: Basic` value, built by the shared `ikigai-http` helper. The
/// password comes from `ikigai-secret` by name at call time; it is never stored
/// in personal.toml. The error names the rule broken (its stable code) and never
/// echoes the user id or password.
pub fn basic_auth(user: &str, password: &str) -> Result<String, String> {
    ikigai_http::auth::basic(user, password)
        .map(ikigai_http::auth::Authorization::into_string)
        .map_err(|e| format!("caldav credentials refused: {}", e.code()))
}

/// PROPFIND (Depth 1) on the calendar home: lists calendars.
pub fn propfind_calendars(home: &str) -> DavRequest {
    DavRequest {
        method: "PROPFIND",
        path: home.into(),
        headers: xml_headers("1"),
        body: r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:displayname/><d:resourcetype/><c:supported-calendar-component-set/></d:prop></d:propfind>"#.into(),
    }
}

/// REPORT calendar-query over `[start, end)` epoch seconds, with the server
/// expanding recurrences into instances.
pub fn report_calendar_query(calendar: &str, start_epoch: i64, end_epoch: i64) -> DavRequest {
    let (s, e) = (ical_utc(start_epoch), ical_utc(end_epoch));
    DavRequest {
        method: "REPORT",
        path: calendar.into(),
        headers: xml_headers("1"),
        body: format!(
            r#"<?xml version="1.0" encoding="utf-8"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data><c:expand start="{s}" end="{e}"/></c:calendar-data></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="{s}" end="{e}"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#
        ),
    }
}

/// RFC 6578 sync-collection REPORT; `token` is empty for the initial sync.
pub fn sync_collection(calendar: &str, token: &str) -> DavRequest {
    DavRequest {
        method: "REPORT",
        path: calendar.into(),
        headers: xml_headers("1"),
        body: format!(
            r#"<?xml version="1.0" encoding="utf-8"?><d:sync-collection xmlns:d="DAV:"><d:sync-token>{}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>"#,
            xml_escape(token)
        ),
    }
}

/// Resource path of an event inside a calendar collection. The uid is
/// percent-encoded as one path segment.
pub fn event_path(calendar: &str, uid: &str) -> String {
    format!(
        "{}/{}.ics",
        calendar.trim_end_matches('/'),
        utf8_percent_encode(uid, SEGMENT)
    )
}

/// PUT that replaces the event at its uid's path only while it is still the
/// version read (`If-Match: <etag>`), or unconditionally when the server gave no
/// entity tag.
#[allow(clippy::too_many_arguments)]
pub fn replace_event(
    calendar: &str,
    uid: &str,
    etag: Option<&str>,
    title: &str,
    start_epoch: i64,
    end_epoch: i64,
    all_day: bool,
    location: Option<&str>,
    description: Option<&str>,
    alerts: &[u32],
) -> DavRequest {
    let mut request = put_event(
        calendar,
        uid,
        title,
        start_epoch,
        end_epoch,
        all_day,
        location,
        description,
        alerts,
    );
    request.headers.retain(|(name, _)| name != "If-None-Match");
    if let Some(etag) = etag {
        request.headers.push(("If-Match".into(), etag.into()));
    }
    request
}

/// PUT a new event (`If-None-Match: *` so it never overwrites).
#[allow(clippy::too_many_arguments)]
pub fn put_event(
    calendar: &str,
    uid: &str,
    title: &str,
    start_epoch: i64,
    end_epoch: i64,
    all_day: bool,
    location: Option<&str>,
    description: Option<&str>,
    alerts: &[u32],
) -> DavRequest {
    DavRequest {
        method: "PUT",
        path: event_path(calendar, uid),
        headers: vec![
            ("Content-Type".into(), "text/calendar; charset=utf-8".into()),
            ("If-None-Match".into(), "*".into()),
        ],
        body: build_ics(
            uid,
            title,
            start_epoch,
            end_epoch,
            all_day,
            location,
            description,
            alerts,
        ),
    }
}

pub fn delete_event(calendar: &str, uid: &str) -> DavRequest {
    DavRequest {
        method: "DELETE",
        path: event_path(calendar, uid),
        headers: vec![],
        body: String::new(),
    }
}

fn ical_utc(epoch: i64) -> String {
    Utc.timestamp_opt(epoch, 0)
        .single()
        .unwrap_or_default()
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

fn ical_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
}

fn ical_unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// The iCalendar body for a new event. The UID is the caller's stamp and the
/// URL carries `urn:event:{uid}`, matching the EventKit backend's identity.
#[allow(clippy::too_many_arguments)]
pub fn build_ics(
    uid: &str,
    title: &str,
    start_epoch: i64,
    end_epoch: i64,
    all_day: bool,
    location: Option<&str>,
    description: Option<&str>,
    alerts: &[u32],
) -> String {
    let mut l = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".into(),
        "PRODID:-//ikigai//ikigai-personal//EN".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{}", ical_escape(uid)),
        format!("DTSTAMP:{}", ical_utc(Utc::now().timestamp())),
        format!("SUMMARY:{}", ical_escape(title)),
        format!("URL:urn:event:{uid}"),
    ];
    if all_day {
        let d = |e: i64| {
            Local
                .timestamp_opt(e, 0)
                .single()
                .unwrap_or_default()
                .format("%Y%m%d")
                .to_string()
        };
        l.push(format!("DTSTART;VALUE=DATE:{}", d(start_epoch)));
        l.push(format!("DTEND;VALUE=DATE:{}", d(end_epoch)));
    } else {
        l.push(format!("DTSTART:{}", ical_utc(start_epoch)));
        l.push(format!("DTEND:{}", ical_utc(end_epoch)));
    }
    if let Some(v) = location {
        l.push(format!("LOCATION:{}", ical_escape(v)));
    }
    if let Some(v) = description {
        l.push(format!("DESCRIPTION:{}", ical_escape(v)));
    }
    for m in alerts {
        l.push("BEGIN:VALARM".into());
        l.push("ACTION:DISPLAY".into());
        l.push("DESCRIPTION:Reminder".into());
        l.push(format!("TRIGGER:-PT{m}M"));
        l.push("END:VALARM".into());
    }
    l.push("END:VEVENT".into());
    l.push("END:VCALENDAR".into());
    l.join("\r\n") + "\r\n"
}

// ---------------------------------------------------------------- XML ----

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_unescape(s: &str) -> String {
    let s = s.trim();
    if let Some(c) = s
        .strip_prefix("<![CDATA[")
        .and_then(|r| r.strip_suffix("]]>"))
    {
        return c.to_string();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#13;", "\r")
        .replace("&amp;", "&")
}

/// Blocks `<name ...>inner</name>` ignoring namespace prefixes; returns inner
/// text of each. Not nested-aware for same-named elements (none occur here).
fn blocks<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let mut out = vec![];
    let mut pos = 0;
    while let Some(rel) = xml[pos..].find('<') {
        let open = pos + rel;
        let Some(end) = xml[open..].find('>') else {
            break;
        };
        let tag_end = open + end;
        let tag = &xml[open + 1..tag_end];
        pos = tag_end + 1;
        if tag.starts_with('/') || tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        let tname = tag
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        if tname.rsplit(':').next() != Some(name) {
            continue;
        }
        if tag.ends_with('/') {
            out.push("");
            continue;
        }
        // find the matching close tag (any prefix)
        let mut scan = pos;
        while let Some(r) = xml[scan..].find("</") {
            let c = scan + r;
            let Some(ce) = xml[c..].find('>') else { break };
            let cname = xml[c + 2..c + ce].trim();
            if cname.rsplit(':').next() == Some(name) {
                out.push(&xml[pos..c]);
                pos = c + ce + 1;
                break;
            }
            scan = c + ce + 1;
        }
    }
    out
}

fn has_element(xml: &str, name: &str) -> bool {
    !blocks(xml, name).is_empty()
}

fn text_of(xml: &str, name: &str) -> Option<String> {
    blocks(xml, name).first().map(|b| xml_unescape(b))
}

/// One `<response>` of a multistatus answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DavResponse {
    pub href: String,
    /// HTTP status of the response (e.g. 404 for a removed member in a sync).
    pub status: u16,
    pub displayname: Option<String>,
    pub is_calendar: bool,
    pub etag: Option<String>,
    pub calendar_data: Option<String>,
}

/// Parse a 207 multistatus body. Also returns the `sync-token`, if present.
pub fn parse_multistatus(xml: &str) -> (Vec<DavResponse>, Option<String>) {
    let responses = blocks(xml, "response")
        .into_iter()
        .map(|r| {
            let status_of = |s: &str| {
                s.split_whitespace()
                    .nth(1)
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(200)
            };
            let status = text_of(r, "status").map(|s| status_of(&s)).unwrap_or(200);
            // A member-level status (sync removal) sits outside propstat.
            let prop = blocks(r, "prop").into_iter().next().unwrap_or("");
            let rt = blocks(prop, "resourcetype")
                .into_iter()
                .next()
                .unwrap_or("");
            DavResponse {
                href: text_of(r, "href").unwrap_or_default(),
                status,
                displayname: text_of(prop, "displayname").filter(|s| !s.is_empty()),
                is_calendar: has_element(rt, "calendar"),
                etag: text_of(prop, "getetag"),
                calendar_data: text_of(prop, "calendar-data"),
            }
        })
        .collect();
    // The token is the multistatus-level element, after the responses.
    let tail = xml.rfind("</").map(|_| xml).unwrap_or(xml);
    let token = blocks(tail, "sync-token")
        .last()
        .map(|t| xml_unescape(t))
        .filter(|t| !t.is_empty());
    (responses, token)
}

/// Calendars out of a PROPFIND answer; `account` labels them.
pub fn parse_calendars(xml: &str, account: &str) -> Vec<CalendarInfo> {
    parse_multistatus(xml)
        .0
        .into_iter()
        .filter(|r| r.is_calendar && r.status < 300)
        .map(|r| CalendarInfo {
            title: r.displayname.unwrap_or_else(|| {
                r.href
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_string()
            }),
            account: account.to_string(),
        })
        .collect()
}

/// Events out of a calendar-query answer.
pub fn parse_events(xml: &str, calendar: &str) -> Vec<EventInfo> {
    parse_multistatus(xml)
        .0
        .iter()
        .filter_map(|r| r.calendar_data.as_deref())
        .flat_map(|ics| parse_ics(ics, calendar))
        .collect()
}

// ------------------------------------------------------------ iCalendar ----

struct Prop {
    name: String,
    params: String,
    value: String,
}

fn unquoted(delimiter: char) -> impl FnMut(char) -> bool {
    let mut quoted = false;
    move |c| {
        if c == '"' {
            quoted = !quoted;
        }
        c == delimiter && !quoted
    }
}

fn unfold(ics: &str) -> Vec<Prop> {
    let mut lines: Vec<String> = vec![];
    for raw in ics.replace("\r\n", "\n").split('\n') {
        if (raw.starts_with(' ') || raw.starts_with('\t')) && !lines.is_empty() {
            lines.last_mut().unwrap().push_str(&raw[1..]);
        } else if !raw.is_empty() {
            lines.push(raw.to_string());
        }
    }
    lines
        .into_iter()
        .filter_map(|l| {
            let (head, value) = l.split_once(unquoted(':'))?;
            let (name, params) = head.split_once(';').unwrap_or((head, ""));
            Some(Prop {
                name: name.to_ascii_uppercase(),
                params: params.to_string(),
                value: value.to_string(),
            })
        })
        .collect()
}

fn local_rfc3339(n: NaiveDateTime) -> String {
    Local
        .from_local_datetime(&n)
        .earliest()
        .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| n.format("%Y-%m-%dT%H:%M:%S").to_string())
}

/// The IANA zone a `TZID=` parameter names, if it names one the tz database knows.
fn zone(p: &Prop) -> Option<chrono_tz::Tz> {
    p.params
        .split(unquoted(';'))
        .filter_map(|kv| kv.split_once('='))
        .find(|(key, _)| key.eq_ignore_ascii_case("TZID"))
        .and_then(|(_, value)| value.trim_matches('"').parse().ok())
}

/// A wall-clock time in `zone` (or local floating time without one) as a local
/// RFC 3339 timestamp. A time skipped by a DST change is read just after the gap.
fn wall_rfc3339(n: NaiveDateTime, zone: Option<chrono_tz::Tz>) -> String {
    match zone {
        Some(tz) => tz
            .from_local_datetime(&n)
            .earliest()
            .or_else(|| tz.from_local_datetime(&(n + Duration::hours(1))).earliest())
            .map(|t| {
                t.with_timezone(&Local)
                    .to_rfc3339_opts(SecondsFormat::Secs, true)
            })
            .unwrap_or_else(|| local_rfc3339(n)),
        None => local_rfc3339(n),
    }
}

/// (rfc3339, is_date_only)
fn parse_time(p: &Prop) -> Option<(String, bool)> {
    let v = p.value.trim();
    if p.params.contains("VALUE=DATE") && !p.params.contains("DATE-TIME") || v.len() == 8 {
        let d = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        return Some((local_rfc3339(d.and_hms_opt(0, 0, 0)?), true));
    }
    if let Some(z) = v.strip_suffix('Z') {
        let n = NaiveDateTime::parse_from_str(z, "%Y%m%dT%H%M%S").ok()?;
        let t = Utc.from_utc_datetime(&n).with_timezone(&Local);
        return Some((t.to_rfc3339_opts(SecondsFormat::Secs, true), false));
    }
    let n = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    Some((wall_rfc3339(n, zone(p)), false))
}

/// Positive iCalendar duration split into nominal days and elapsed time.
fn duration_parts(value: &str) -> Option<(Days, Duration)> {
    let value = value.trim();
    let value = value.strip_prefix('+').unwrap_or(value).strip_prefix('P')?;
    let (days, seconds) = if let Some(weeks) = value.strip_suffix('W') {
        (weeks.parse::<u64>().ok()?.checked_mul(7)?, 0)
    } else {
        let (date, time) = match value.split_once('T') {
            Some((_, "")) => return None,
            Some(parts) => parts,
            None => (value, ""),
        };
        let days = match date.strip_suffix('D') {
            Some(days) => days.parse::<u64>().ok()?,
            None if date.is_empty() => 0,
            None => return None,
        };
        let mut rest = time;
        let mut seconds = 0i64;
        for (unit, factor) in [('H', 3600), ('M', 60), ('S', 1)] {
            if let Some((number, tail)) = rest.split_once(unit) {
                let number = i64::try_from(number.parse::<u64>().ok()?).ok()?;
                seconds = seconds.checked_add(number.checked_mul(factor)?)?;
                rest = tail;
            }
        }
        if !rest.is_empty() {
            return None;
        }
        (days, seconds)
    };
    if days == 0 && seconds == 0 {
        return None;
    }
    Some((Days::new(days), Duration::try_seconds(seconds)?))
}

fn duration_end(start_prop: &Prop, all_day: bool, value: &str) -> Option<String> {
    let (days, seconds) = duration_parts(value)?;
    let start_value = start_prop.value.trim();
    let start = start_value;
    if all_day {
        if value.contains('T') {
            return None;
        }
        let date = NaiveDate::parse_from_str(start, "%Y%m%d").ok()?;
        return Some(local_rfc3339(
            date.checked_add_days(days)?.and_hms_opt(0, 0, 0)?,
        ));
    }
    if let Some(utc) = start.strip_suffix('Z') {
        let start = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        let end = start.checked_add_days(days)?.checked_add_signed(seconds)?;
        return Some(
            Utc.from_utc_datetime(&end)
                .with_timezone(&Local)
                .to_rfc3339_opts(SecondsFormat::Secs, true),
        );
    }
    // Apply nominal days in calendar time before adding elapsed hours/minutes/seconds.
    let start = NaiveDateTime::parse_from_str(start_value, "%Y%m%dT%H%M%S").ok()?;
    let end_day = start.checked_add_days(days)?;
    let wall = wall_rfc3339(end_day, zone(start_prop));
    if seconds.is_zero() {
        return Some(wall);
    }
    let end = chrono::DateTime::parse_from_rfc3339(&wall)
        .ok()?
        .checked_add_signed(seconds)?;
    Some(
        end.with_timezone(&Local)
            .to_rfc3339_opts(SecondsFormat::Secs, true),
    )
}

fn trigger_minutes(v: &str) -> Option<u32> {
    let rest = v.strip_prefix('-')?.strip_prefix('P')?;
    let (days, time) = match rest.split_once('T') {
        Some((d, t)) => (d, t),
        None => (rest, ""),
    };
    let mut mins = 0u32;
    if let Some(d) = days.strip_suffix('D') {
        mins += d.parse::<u32>().ok()? * 1440;
    }
    let mut num = String::new();
    for c in time.chars() {
        match c {
            '0'..='9' => num.push(c),
            'H' => mins += std::mem::take(&mut num).parse::<u32>().ok()? * 60,
            'M' => mins += std::mem::take(&mut num).parse::<u32>().ok()?,
            'S' => num.clear(),
            _ => return None,
        }
    }
    Some(mins)
}

/// Every VEVENT in an iCalendar document as an [`EventInfo`]. Recurring events
/// (RRULE, or expanded instances carrying RECURRENCE-ID) get per-occurrence
/// uids via `occurrence_uid`; cancelled events are dropped.
pub fn parse_ics(ics: &str, calendar: &str) -> Vec<EventInfo> {
    let props = unfold(ics);
    let mut out = vec![];
    let mut cur: Option<Vec<&Prop>> = None;
    let mut in_alarm = false;
    for p in &props {
        match (p.name.as_str(), p.value.as_str()) {
            ("BEGIN", "VEVENT") => cur = Some(vec![]),
            ("END", "VEVENT") => {
                if let Some(ev) = cur.take().and_then(|v| build_event(&v, calendar)) {
                    out.push(ev);
                }
            }
            ("BEGIN", "VALARM") => in_alarm = true,
            ("END", "VALARM") => in_alarm = false,
            // DURATION, ATTENDEE, and other alarm fields belong to the alarm.
            _ if !in_alarm || p.name == "TRIGGER" => {
                if let Some(c) = cur.as_mut() {
                    c.push(p);
                }
            }
            _ => {}
        }
    }
    out
}

/// What a published iCalendar feed (a free/busy address) says about busy time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Feed {
    /// Every VEVENT, and every VFREEBUSY period as an untitled event.
    pub events: Vec<EventInfo>,
    /// Recurring events (an RRULE or RDATE on a VEVENT) whose later occurrences are
    /// NOT in `events`: a feed is not expanded here, so only each one's first
    /// occurrence is. Zero for a VFREEBUSY feed, which the server expands.
    pub unexpanded: usize,
}

/// Parse a published feed: the VEVENTs of a calendar export (Google's secret
/// iCal address, an iCloud public calendar), and the FREEBUSY periods of a
/// VFREEBUSY answer (a Fastmail free/busy link). `calendar` labels the events.
pub fn parse_feed(ics: &str, calendar: &str) -> Feed {
    let mut events = parse_ics(ics, calendar);
    events.extend(parse_freebusy(ics, calendar));
    Feed {
        events,
        unexpanded: count_recurring(ics),
    }
}

/// VEVENTs that recur by rule or by date list (an override, RECURRENCE-ID, is
/// one occurrence and does not count).
fn count_recurring(ics: &str) -> usize {
    let mut count = 0;
    let mut recurs = false;
    let mut in_event = false;
    for p in unfold(ics) {
        match (p.name.as_str(), p.value.trim()) {
            ("BEGIN", "VEVENT") => {
                in_event = true;
                recurs = false;
            }
            ("END", "VEVENT") => {
                if in_event && recurs {
                    count += 1;
                }
                in_event = false;
            }
            ("RRULE", _) | ("RDATE", _) if in_event => recurs = true,
            _ => {}
        }
    }
    count
}

/// One FREEBUSY period, `start/end` or `start/duration`, as unix seconds. RFC
/// 5545 writes these in UTC; a period without the `Z` is read as local time.
fn freebusy_period(value: &str) -> Option<(i64, i64)> {
    let (start, rest) = value.trim().split_once('/')?;
    let epoch = |v: &str| -> Option<i64> {
        let v = v.trim();
        match v.strip_suffix('Z') {
            Some(z) => Some(
                Utc.from_utc_datetime(&NaiveDateTime::parse_from_str(z, "%Y%m%dT%H%M%S").ok()?)
                    .timestamp(),
            ),
            None => Some(
                Local
                    .from_local_datetime(&NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?)
                    .earliest()?
                    .timestamp(),
            ),
        }
    };
    let start = epoch(start)?;
    let end = if rest.trim_start().starts_with(['P', '+']) {
        let (_, elapsed) = duration_parts(rest)?;
        start
            .checked_add(duration_days(rest)?.checked_mul(86_400)?)?
            .checked_add(elapsed.num_seconds())?
    } else {
        epoch(rest)?
    };
    (end > start).then_some((start, end))
}

/// The whole days of a duration (`P2D`, `P1W`), for a FREEBUSY period, where a
/// day is 86,400 seconds because the period is in UTC.
fn duration_days(value: &str) -> Option<i64> {
    let value = value.trim();
    let value = value.strip_prefix('+').unwrap_or(value).strip_prefix('P')?;
    if let Some(weeks) = value.strip_suffix('W') {
        return weeks.parse::<i64>().ok()?.checked_mul(7);
    }
    let date = value.split('T').next().unwrap_or("");
    match date.strip_suffix('D') {
        Some(days) => days.parse().ok(),
        None => Some(0),
    }
}

/// The busy periods of every VFREEBUSY component. FBTYPE=FREE is dropped; BUSY,
/// BUSY-TENTATIVE, BUSY-UNAVAILABLE and an absent FBTYPE are busy.
pub fn parse_freebusy(ics: &str, calendar: &str) -> Vec<EventInfo> {
    let rfc = |epoch: i64| {
        Local
            .timestamp_opt(epoch, 0)
            .single()
            .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true))
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    let mut inside = false;
    for p in unfold(ics) {
        match (p.name.as_str(), p.value.trim()) {
            ("BEGIN", "VFREEBUSY") => inside = true,
            ("END", "VFREEBUSY") => inside = false,
            ("FREEBUSY", periods) if inside => {
                let free = p
                    .params
                    .split(unquoted(';'))
                    .filter_map(|kv| kv.split_once('='))
                    .any(|(k, v)| {
                        k.eq_ignore_ascii_case("FBTYPE") && v.eq_ignore_ascii_case("FREE")
                    });
                if free {
                    continue;
                }
                for (start, end) in periods.split(',').filter_map(freebusy_period) {
                    out.push(EventInfo {
                        uid: format!("freebusy-{start}-{end}"),
                        title: String::new(),
                        calendar: calendar.to_string(),
                        start: rfc(start),
                        end: rfc(end),
                        all_day: false,
                        busy: true,
                        location: None,
                        description: None,
                        url: None,
                        attendees: Vec::new(),
                        alerts: Vec::new(),
                    });
                }
            }
            _ => {}
        }
    }
    out
}

fn build_event(ps: &[&Prop], calendar: &str) -> Option<EventInfo> {
    let get = |n: &str| ps.iter().find(|p| p.name == n).copied();
    let val = |n: &str| {
        get(n)
            .map(|p| ical_unescape(&p.value))
            .filter(|s| !s.is_empty())
    };
    if val("STATUS").is_some_and(|s| s.eq_ignore_ascii_case("CANCELLED")) {
        return None;
    }
    let uid = val("UID")?;
    let dtstart = get("DTSTART")?;
    let (start, all_day) = parse_time(dtstart)?;
    let end = get("DTEND")
        .and_then(parse_time)
        .map(|(end, _)| end)
        .or_else(|| get("DURATION").and_then(|p| duration_end(dtstart, all_day, &p.value)))
        .or_else(|| {
            if all_day {
                duration_end(dtstart, true, "P1D")
            } else {
                None
            }
        })
        .unwrap_or_else(|| start.clone());
    let recurrence_id = get("RECURRENCE-ID");
    let recurring = get("RRULE").is_some() || recurrence_id.is_some();
    let original_start = recurrence_id.and_then(parse_time).map(|(value, _)| value);
    let mut alerts: Vec<u32> = ps
        .iter()
        .filter(|p| p.name == "TRIGGER")
        .filter_map(|p| trigger_minutes(p.value.trim()))
        .collect();
    alerts.sort_unstable();
    let url = val("URL").filter(|u| !u.starts_with("urn:event:"));
    let attendees = ps
        .iter()
        .filter(|p| p.name == "ATTENDEE")
        .map(|p| {
            p.params
                .split(unquoted(';'))
                .filter_map(|kv| kv.split_once('='))
                .find_map(|(key, value)| key.eq_ignore_ascii_case("CN").then_some(value))
                .map(|cn| cn.trim_matches('"').to_string())
                .unwrap_or_else(|| {
                    let v = p.value.as_str();
                    v.strip_prefix("mailto:").unwrap_or(v).to_string()
                })
        })
        .collect();
    Some(EventInfo {
        uid: occurrence_uid(&uid, recurring, original_start.as_deref().unwrap_or(&start)),
        title: val("SUMMARY").unwrap_or_default(),
        calendar: calendar.to_string(),
        start,
        end,
        all_day,
        busy: !get("TRANSP").is_some_and(|p| p.value.trim().eq_ignore_ascii_case("TRANSPARENT")),
        location: val("LOCATION"),
        description: val("DESCRIPTION"),
        url,
        attendees,
        alerts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(ics: &str) -> String {
        format!(
            r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:response><D:href>/cal/a.ics</D:href><D:propstat><D:prop><D:getetag>"e1"</D:getetag><C:calendar-data><![CDATA[{ics}]]></C:calendar-data></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#
        )
    }

    #[test]
    fn lists_calendars() {
        let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
<d:response><d:href>/cal/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/cal/work/</d:href><d:propstat><d:prop><d:displayname>Work &amp; Co</d:displayname><d:resourcetype><d:collection/><c:calendar/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
        assert_eq!(
            parse_calendars(xml, "Fastmail"),
            vec![CalendarInfo {
                title: "Work & Co".into(),
                account: "Fastmail".into()
            }]
        );
    }

    #[test]
    fn all_day_event_end_is_exclusive_date() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:ad1\r\nSUMMARY:Holiday\r\nDTSTART;VALUE=DATE:20260704\r\nDTEND;VALUE=DATE:20260705\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let ev = parse_events(&wrap(ics), "Home");
        assert_eq!(ev.len(), 1);
        assert!(ev[0].all_day && ev[0].busy);
        assert!(ev[0].start.starts_with("2026-07-04T00:00:00"));
        assert!(ev[0].end.starts_with("2026-07-05T00:00:00"));
        assert_eq!(ev[0].uid, "ad1");
    }

    #[test]
    fn transparent_event_is_not_busy() {
        let ics = "BEGIN:VEVENT\nUID:t1\nSUMMARY:Birthday\nDTSTART;VALUE=DATE:20260301\nTRANSP:TRANSPARENT\nEND:VEVENT\n";
        let ev = parse_ics(ics, "B");
        assert!(!ev[0].busy);
        assert!(ev[0].end.starts_with("2026-03-02")); // missing DTEND → +1 day
    }

    #[test]
    fn recurring_instances_get_distinct_uids() {
        let one = |day: &str| {
            format!("BEGIN:VEVENT\nUID:r1\nRECURRENCE-ID:{day}T170000Z\nSUMMARY:Standup\nDTSTART:{day}T170000Z\nDTEND:{day}T173000Z\nRRULE:FREQ=DAILY\nEND:VEVENT\n")
        };
        let ics = format!("{}{}", one("20260701"), one("20260702"));
        let ev = parse_ics(&ics, "W");
        assert_eq!(ev.len(), 2);
        assert_ne!(ev[0].uid, ev[1].uid);
        assert!(ev[0].uid.starts_with("r1-"));
    }

    #[test]
    fn put_body_reads_back_as_same_subject() {
        let req = put_event(
            "/cal/work/",
            "abc-123",
            "Plan, review; ship",
            1_782_900_000,
            1_782_903_600,
            false,
            Some("Room 1"),
            Some("line1\nline2"),
            &[15, 60],
        );
        assert_eq!(req.method, "PUT");
        assert_eq!(req.path, "/cal/work/abc-123.ics");
        let ev = parse_ics(&req.body, "Work");
        assert_eq!(ev.len(), 1);
        let e = &ev[0];
        assert_eq!(e.uid, "abc-123"); // urn:event:abc-123
        assert_eq!(e.title, "Plan, review; ship");
        assert_eq!(e.location.as_deref(), Some("Room 1"));
        assert_eq!(e.description.as_deref(), Some("line1\nline2"));
        assert_eq!(e.alerts, vec![15, 60]);
        assert_eq!(e.url, None); // identity token is not a real URL
        let secs = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp();
        assert_eq!(
            (secs(&e.start), secs(&e.end)),
            (1_782_900_000, 1_782_903_600)
        );
    }

    #[test]
    fn unfolds_attendees_cancelled_and_url() {
        let ics = "BEGIN:VEVENT\nUID:x\nSUMMARY:Long\n  title\nDTSTART:20260701T100000\nDTEND:20260701T110000\nATTENDEE;CN=\"Ann\":mailto:ann@x.org\nATTENDEE:mailto:bob@x.org\nURL:https://teams.example/j\nEND:VEVENT\nBEGIN:VEVENT\nUID:y\nSTATUS:CANCELLED\nDTSTART:20260701T100000\nEND:VEVENT\n";
        let ev = parse_ics(ics, "C");
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].title, "Long title");
        assert_eq!(ev[0].attendees, vec!["Ann", "bob@x.org"]);
        assert_eq!(ev[0].url.as_deref(), Some("https://teams.example/j"));
    }

    #[test]
    fn quoted_attendee_parameters_keep_colons_and_semicolons() {
        let ics = "BEGIN:VEVENT\r\nUID:quoted\r\nDTSTART:20260701T100000Z\r\nATTENDEE;CN=\"Team: Blue; West\":mailto:team@example.test\r\nEND:VEVENT\r\n";
        assert_eq!(
            parse_events(&wrap(ics), "Synthetic")[0].attendees,
            ["Team: Blue; West"]
        );
    }

    #[test]
    fn expanded_event_uses_duration_for_its_end() {
        let ics = concat!(
            "BEGIN:VEVENT\r\nUID:series\r\nRECURRENCE-ID:20260701T100000Z\r\n",
            "DTSTART:20260701T100000Z\r\nDURATION:PT1H15M30S\r\n",
            "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\n",
            "TRIGGER:-PT15M\r\nDURATION:PT5M\r\nREPEAT:2\r\nEND:VALARM\r\n",
            "END:VEVENT\r\n",
        );
        let events = parse_events(&wrap(ics), "Synthetic");
        let epoch = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp();
        assert_eq!(epoch(&events[0].end) - epoch(&events[0].start), 4530);
        assert_eq!(events[0].alerts, [15]);
        let instant = ics.replace("DURATION:PT1H15M30S\r\n", "");
        let instant = parse_events(&wrap(&instant), "Synthetic").remove(0);
        assert_eq!(instant.start, instant.end);
    }

    #[test]
    fn all_day_duration_keeps_the_exclusive_end_date() {
        let ics = "BEGIN:VEVENT\nUID:week\nDTSTART;VALUE=DATE:20261101\nDURATION:P1W\nEND:VEVENT\n";
        let events = parse_events(&wrap(ics), "Synthetic");
        assert!(events[0].all_day);
        assert!(events[0].end.starts_with("2026-11-08T00:00:00"));
    }

    #[test]
    fn moving_an_override_keeps_its_occurrence_uid() {
        let ics = "BEGIN:VEVENT\nUID:series\nRECURRENCE-ID;VALUE=DATE:20260701\nDTSTART;VALUE=DATE:20260701\nEND:VEVENT\n";
        let moved = ics.replace("DTSTART;VALUE=DATE:20260701", "DTSTART;VALUE=DATE:20260702");
        let original = parse_events(&wrap(ics), "Synthetic");
        let updated = parse_events(&wrap(&moved), "Synthetic");
        assert_ne!(original[0].start, updated[0].start);
        assert_eq!(original[0].uid, updated[0].uid);
    }

    #[test]
    fn sync_token_and_removals() {
        let xml = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/cal/gone.ics</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response><d:response><d:href>/cal/new.ics</d:href><d:propstat><d:prop><d:getetag>"z"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response><d:sync-token>http://x/sync/42</d:sync-token></d:multistatus>"#;
        let (r, tok) = parse_multistatus(xml);
        assert_eq!(tok.as_deref(), Some("http://x/sync/42"));
        assert_eq!((r[0].status, r[1].status), (404, 200));
        assert_eq!(r[1].etag.as_deref(), Some("\"z\""));
    }

    fn epoch(s: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp()
    }

    #[test]
    fn an_iana_tzid_is_resolved_on_both_sides_of_a_dst_change() {
        // 2026-11-01 is the US fall-back: 09:00 Los Angeles is 16:00Z the day
        // before and 17:00Z the day after, whatever zone this host runs in.
        let ics = concat!(
            "BEGIN:VEVENT\r\nUID:a\r\nDTSTART;TZID=America/Los_Angeles:20261031T090000\r\n",
            "DURATION:PT1H\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:b\r\nDTSTART;TZID=\"America/Los_Angeles\":20261102T090000\r\n",
            "DTEND;TZID=America/Los_Angeles:20261102T100000\r\nEND:VEVENT\r\n",
        );
        let events = parse_ics(ics, "Z");
        assert_eq!(epoch(&events[0].start), epoch("2026-10-31T16:00:00Z"));
        assert_eq!(epoch(&events[0].end), epoch("2026-10-31T17:00:00Z"));
        assert_eq!(epoch(&events[1].start), epoch("2026-11-02T17:00:00Z"));
        assert_eq!(epoch(&events[1].end), epoch("2026-11-02T18:00:00Z"));
    }

    #[test]
    fn a_tzid_the_database_does_not_know_reads_as_local_time() {
        let ics =
            "BEGIN:VEVENT\nUID:w\nDTSTART;TZID=Pacific Standard Time:20261201T090000\nEND:VEVENT\n";
        let events = parse_ics(ics, "Z");
        let local = chrono::Local
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2026, 12, 1)
                    .unwrap()
                    .and_hms_opt(9, 0, 0)
                    .unwrap(),
            )
            .earliest()
            .unwrap()
            .timestamp();
        assert_eq!(epoch(&events[0].start), local);
    }

    #[test]
    fn a_feed_counts_recurring_masters_but_not_overrides() {
        let ics = concat!(
            "BEGIN:VEVENT\nUID:r\nDTSTART:20261201T100000Z\nRRULE:FREQ=DAILY\nEND:VEVENT\n",
            "BEGIN:VEVENT\nUID:r\nRECURRENCE-ID:20261202T100000Z\nDTSTART:20261202T110000Z\nEND:VEVENT\n",
            "BEGIN:VEVENT\nUID:d\nDTSTART:20261201T100000Z\nRDATE:20261205T100000Z\nEND:VEVENT\n",
        );
        assert_eq!(parse_feed(ics, "F").unexpanded, 2);
    }

    #[test]
    fn a_uid_is_one_path_segment() {
        assert_eq!(
            event_path("https://dav.example.test/cal/", "a/b c@d"),
            "https://dav.example.test/cal/a%2Fb%20c%40d.ics"
        );
        assert_eq!(event_path("/c", "s-1_2.3~"), "/c/s-1_2.3~.ics");
    }

    #[test]
    fn requests_and_auth() {
        assert_eq!(
            basic_auth("Aladdin", "open sesame").unwrap(),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
        let err = basic_auth("a:b", "hunter2").unwrap_err();
        assert_eq!(err, "caldav credentials refused: basic-user-id-colon");
        assert!(!err.contains("hunter2"));
        let q = report_calendar_query("/c/", 0, 86400);
        assert_eq!(q.method, "REPORT");
        assert!(q.body.contains(r#"start="19700101T000000Z""#));
        assert!(q.body.contains("expand"));
        assert!(sync_collection("/c/", "t&1").body.contains("t&amp;1"));
        assert_eq!(delete_event("/c", "u").path, "/c/u.ics");
        assert_eq!(propfind_calendars("/h/").headers[1].1, "1");
    }
}
