# Changelog

## 0.10.2

- Invalid optional arguments are now refused with the argument name and reason instead of silently selecting a default. This changes behavior for callers that previously sent invalid values; no persisted state changes are needed.
