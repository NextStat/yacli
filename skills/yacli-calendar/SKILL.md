---
name: yacli-calendar
description: "Yandex Calendar: list calendars, view events, create and delete events via CalDAV. Use when the task involves scheduling."
metadata:
  author: NextStat
---

# yacli calendar

Read `yacli-shared` first for account and auth setup.

## Commands

### List calendars

```
yacli calendar calendars [--account ALIAS]
```

Returns available calendars with names and IDs.

### List events

```
yacli calendar events [FROM] [TO] [--calendar NAME] [--limit N] [--account ALIAS]
```

FROM and TO are dates in `YYYY-MM-DD` format. Defaults: calendar=default, limit=20, window=30 days from today.

Example — events for tomorrow:
```
yacli calendar events 2026-03-14 2026-03-15
```

### Create an event

```
yacli calendar create SUMMARY START END [--calendar NAME] [--description TEXT] [--location TEXT] [--account ALIAS]
```

START and END are ISO 8601 datetime strings: `2026-03-14T10:00:00`.

Example:
```
yacli calendar create "Team sync" 2026-03-14T10:00:00 2026-03-14T11:00:00 --description "Weekly standup"
```

### Delete an event

```
yacli calendar delete UID [--calendar NAME] [--account ALIAS]
```

UID comes from the `uid` field in `calendar events` output.

## Typical flow

1. `yacli calendar calendars` — see available calendars
2. `yacli calendar events` — view upcoming events
3. `yacli calendar create ...` — add an event
