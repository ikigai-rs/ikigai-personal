//! Sans-IO CalDAV core: builds the WebDAV requests a calendar backend needs and
//! parses the answers into [`CalendarInfo`] / [`EventInfo`]. No socket, no HTTP
//! client — the caller carries a [`DavRequest`] over the kernel's `urn:http`
//! (so `urn:cap:net:<host>` gates and records every call) and hands the body
//! back to a parser here.
//!
//! Limits: `TZID`-qualified times are read as local floating time (no tz
//! database); `DURATION` ends are not interpreted; vCard/CardDAV is not covered.
//! Not yet exercised against a real iCloud/Fastmail/Nextcloud/Radicale server.

use crate::platform::{occurrence_uid, CalendarInfo, EventInfo};
use chrono::{Duration, Local, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};

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

/// `Authorization: Basic` value. The password comes from `ikigai-secret` by
/// name at call time; it is never stored in personal.toml.
pub fn basic_auth(user: &str, password: &str) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let data = format!("{user}:{password}").into_bytes();
    let mut out = String::from("Basic ");
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
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

/// Resource path of an event inside a calendar collection.
pub fn event_path(calendar: &str, uid: &str) -> String {
    format!("{}/{}.ics", calendar.trim_end_matches('/'), uid)
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
            let colon = l.find(':')?;
            // Colons inside quoted params are rare; the first colon splits.
            let (head, value) = (&l[..colon], &l[colon + 1..]);
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
    Some((local_rfc3339(n), false))
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
    for p in &props {
        match (p.name.as_str(), p.value.as_str()) {
            ("BEGIN", "VEVENT") => cur = Some(vec![]),
            ("END", "VEVENT") => {
                if let Some(ev) = cur.take().and_then(|v| build_event(&v, calendar)) {
                    out.push(ev);
                }
            }
            _ => {
                if let Some(c) = cur.as_mut() {
                    c.push(p);
                }
            }
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
    let (start, all_day) = parse_time(get("DTSTART")?)?;
    let end = match get("DTEND").and_then(parse_time) {
        Some((e, _)) => e,
        None if all_day => {
            let d = chrono::DateTime::parse_from_rfc3339(&start)
                .ok()?
                .naive_local()
                + Duration::days(1);
            local_rfc3339(d)
        }
        None => start.clone(),
    };
    let recurring = get("RRULE").is_some() || get("RECURRENCE-ID").is_some();
    // Alarms: only the TRIGGERs between BEGIN:VALARM … END:VALARM; the flat
    // property list keeps order, so take TRIGGERs (VEVENT has no other).
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
                .split(';')
                .find_map(|kv| kv.strip_prefix("CN="))
                .map(|cn| cn.trim_matches('"').to_string())
                .unwrap_or_else(|| {
                    let v = p.value.as_str();
                    v.strip_prefix("mailto:").unwrap_or(v).to_string()
                })
        })
        .collect();
    Some(EventInfo {
        uid: occurrence_uid(&uid, recurring, &start),
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
    fn sync_token_and_removals() {
        let xml = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/cal/gone.ics</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response><d:response><d:href>/cal/new.ics</d:href><d:propstat><d:prop><d:getetag>"z"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response><d:sync-token>http://x/sync/42</d:sync-token></d:multistatus>"#;
        let (r, tok) = parse_multistatus(xml);
        assert_eq!(tok.as_deref(), Some("http://x/sync/42"));
        assert_eq!((r[0].status, r[1].status), (404, 200));
        assert_eq!(r[1].etag.as_deref(), Some("\"z\""));
    }

    #[test]
    fn requests_and_auth() {
        assert_eq!(
            basic_auth("Aladdin", "open sesame"),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
        let q = report_calendar_query("/c/", 0, 86400);
        assert_eq!(q.method, "REPORT");
        assert!(q.body.contains(r#"start="19700101T000000Z""#));
        assert!(q.body.contains("expand"));
        assert!(sync_collection("/c/", "t&1").body.contains("t&amp;1"));
        assert_eq!(delete_event("/c", "u").path, "/c/u.ics");
        assert_eq!(propfind_calendars("/h/").headers[1].1, "1");
    }
}
