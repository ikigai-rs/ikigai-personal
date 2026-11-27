# Changelog

## Unreleased

- `urn:personal:calendar:event:uid:{uid}` puts one event by its stable uid into a named calendar: the event carrying that uid is updated in place (a moved session moves its entry), or created when the calendar does not hold it. Delete removes it. A put replaces every field it states; an absent location, description or alert list clears the stored one. Additive: `Sink urn:personal:calendar` keeps creating, and no persisted state changes (ledger #4523).

## 0.10.2

- Invalid optional arguments are now refused with the argument name and reason instead of silently selecting a default. This changes behavior for callers that previously sent invalid values; no persisted state changes are needed.
