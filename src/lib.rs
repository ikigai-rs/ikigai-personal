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
    Description, EndpointSpace, Error, Exact, FnEndpoint, Invocation, ReprType, Representation,
    Result, Verb,
};

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

/// The personal-contexts space: binds the resources at `urn:personal:*`.
pub fn space() -> EndpointSpace {
    EndpointSpace::new()
        .bind(Exact::new("urn:personal:contacts"), contacts())
        .bind(Exact::new("urn:personal:calendar"), calendar())
        .bind(Exact::new("urn:personal:availability"), availability())
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
        let kernel = Kernel::new(Arc::new(space()));
        block_on(kernel.issue(
            Request::new(Verb::Source, Iri::parse(iri).unwrap()),
            capability,
        ))
    }

    fn text(iri: &str, capability: &Capability) -> String {
        String::from_utf8(source(iri, capability).unwrap().bytes).unwrap()
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
