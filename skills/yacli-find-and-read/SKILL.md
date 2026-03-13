---
name: yacli-find-and-read
description: "Find and read an email: search by keyword, then read the matching message by UID. Use when looking for a specific email."
metadata:
  author: NextStat
---

# Find and read

A two-step workflow to locate and read a specific email.

## Steps

1. Search for the message:

```
yacli mail search "KEYWORD" --limit 5
```

2. Pick the relevant message from results. Note the `uid` field.

3. Read it:

```
yacli mail read UID
```

## Notes

- Search looks in INBOX by default. Use `--folder FOLDER` for other folders.
- If search returns too many results, narrow the query or reduce `--limit`.
- The `uid` from step 1 output is the required argument for step 3.
