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

use ikigai_core::{
    ArgSpec, Description, EndpointSpace, Error, Exact, FnEndpoint, Invocation, ReprType,
    Representation, Result, Verb,
};
use serde::Deserialize;

pub use platform::CalendarInfo;

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

/// `calendar`: the calendar, projected on the capability — full detail with
/// `…:read:detail`, free/busy with `…:read:freebusy`, denied with neither.
pub fn calendar() -> FnEndpoint {
    FnEndpoint::new("calendar", |inv: &Invocation<'_>| {
        if inv
            .capability
            .allows(&read_scope("urn:personal:calendar", Some("detail")))
        {
            resolve("calendar", platform::calendar())
        } else if inv
            .capability
            .allows(&read_scope("urn:personal:calendar", Some("freebusy")))
        {
            // The free/busy projection: busy blocks only, no titles or attendees.
            resolve("calendar", platform::availability())
        } else {
            Err(denied(
                "calendar",
                "urn:cap:personal:calendar:read:detail or :freebusy",
            ))
        }
    })
    .with_description(
        Description::new("calendar")
            .title("Calendar")
            .summary(
                "The calendar. Projects on the capability: full detail (events with titles and \
                 attendees) under `…:read:detail`, or busy blocks only under `…:read:freebusy`.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .output("text/plain;charset=utf-8"),
    )
}

/// `availability`: the free/busy projection of the calendar — a named convenience
/// for the always-minimized view, gated on `urn:cap:personal:calendar:read:freebusy`.
pub fn availability() -> FnEndpoint {
    FnEndpoint::new("availability", |inv: &Invocation<'_>| {
        let scope = read_scope("urn:personal:calendar", Some("freebusy"));
        if !inv.capability.allows(&scope) {
            return Err(denied("availability", &scope));
        }
        resolve("availability", platform::availability())
    })
    .with_description(
        Description::new("availability")
            .title("Availability")
            .summary(
                "Free/busy projection of the calendar — busy blocks only, no titles or \
                 attendees. The same minimized view `calendar` yields under a free/busy capability.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
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

    // Only the macOS-gated tests below read the body out; gate the helper to
    // match, or it's dead code on other platforms (Linux CI) under -D warnings.
    #[cfg(target_os = "macos")]
    fn text(iri: &str, capability: &Capability) -> String {
        String::from_utf8(source(iri, capability).unwrap().bytes).unwrap()
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
        // Only the view is required.
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
        let msg = format!("{:?}", denied.unwrap_err());
        assert!(msg.contains("not authorized"), "{msg}");
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
    fn the_unsupported_platform_message_is_clear() {
        // Platform-agnostic: the `None` branch is the placeholder behaviour.
        let err = resolve("calendar", None).unwrap_err();
        assert!(format!("{err:?}").contains("not supported"));
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

    #[cfg(target_os = "macos")]
    #[test]
    fn root_sees_full_detail() {
        let detail = text("urn:personal:calendar", &Capability::root());
        assert!(detail.contains("Design review")); // a title only detail exposes
        assert!(is_supported());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_freebusy_capability_sees_only_busy_blocks() {
        let freebusy = Capability::root().attenuate(["urn:cap:personal:calendar:read:freebusy"]);
        let out = text("urn:personal:calendar", &freebusy);
        assert!(out.contains("busy"));
        assert!(!out.contains("Design review")); // detail withheld
                                                 // …and that capability cannot reach contacts.
        assert!(source("urn:personal:contacts", &freebusy).is_err());
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
