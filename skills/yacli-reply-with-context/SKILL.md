---
name: yacli-reply-with-context
description: "Reply to an email with calendar context: read the message, check schedule, then reply with relevant information. Use when a reply needs scheduling awareness."
metadata:
  author: NextStat
---

# Reply with context

A multi-step workflow to reply to an email with awareness of your schedule.

## Steps

1. Read the original message:

```
yacli mail read UID
```

2. Check calendar for relevant dates mentioned in the message:

```
yacli calendar events FROM TO
```

3. Compose and send the reply:

```
yacli mail reply UID "Your reply text here"
```

## Notes

- Extract dates from the message body in step 1 to use as FROM/TO in step 2.
- The reply preserves threading headers automatically.
- Use `--cc EMAIL` to add recipients to the reply.
- Use `--html "<p>formatted reply</p>"` for rich-text replies.
