# Changelog

## Unreleased

- A calendar off the machine (ledger #4524): `remote` in the calendar file names, by secret, free/busy iCalendar addresses and one CalDAV calendar. `urn:personal:availability` reads busy blocks from them through `urn:secret:*` and `urn:http:*` under the caller's capability, and a put or delete on `urn:personal:calendar:event:uid:{uid}` naming the CalDAV calendar writes it by uid, with `If-None-Match`/`If-Match` so a concurrent edit is a Conflict. Feeds are read for VEVENTs and VFREEBUSY periods; a recurring event in a feed is not expanded, and the answer says how many. iCalendar times with an IANA `TZID` are now resolved with the tz database (they were read as local time). `CalDAV event_path` percent-encodes the uid. `view` may be absent from the calendar file.
- `urn:personal:calendar:event:uid:{uid}` puts one event by its stable uid into a named calendar: the event carrying that uid is updated in place (a moved session moves its entry), or created when the calendar does not hold it. Delete removes it. A put replaces every field it states; an absent location, description or alert list clears the stored one. Additive: `Sink urn:personal:calendar` keeps creating, and no persisted state changes (ledger #4523).

## 0.10.2

- Invalid optional arguments are now refused with the argument name and reason instead of silently selecting a default. This changes behavior for callers that previously sent invalid values; no persisted state changes are needed.
