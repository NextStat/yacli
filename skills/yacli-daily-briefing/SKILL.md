---
name: yacli-daily-briefing
description: "Daily briefing: show unread inbox messages and today's calendar events in one flow. Use for morning review or status check."
metadata:
  author: NextStat
---

# Daily briefing

A multi-step workflow to get a quick overview of mail and schedule.

## Steps

1. List recent inbox messages:

```
yacli mail list --limit 10
```

2. List today's events (replace dates with actual today/tomorrow):

```
yacli calendar events YYYY-MM-DD YYYY-MM-DD+1
```

3. Summarize: count of messages, any urgent subjects, upcoming meetings.

## Notes

- If multiple accounts exist, run for each account using `--account ALIAS`.
- Adjust `--limit` based on expected volume.
- Use `yacli mail read UID` to drill into any message that looks important.
