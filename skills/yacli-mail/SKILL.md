---
name: yacli-mail
description: "Yandex Mail: search, read, send, reply, and forward emails via IMAP/SMTP. Use when the task involves email."
metadata:
  author: NextStat
---

# yacli mail

Read `yacli-shared` first for account and auth setup.

## Commands

### List folders

```
yacli mail folders [--account ALIAS]
```

### List messages

```
yacli mail list [--folder FOLDER] [--limit N] [--account ALIAS]
```

Defaults: folder=INBOX, limit=20. Returns array of messages with `uid`, `from`, `subject`, `date`.

### Search messages

```
yacli mail search QUERY [--folder FOLDER] [--limit N] [--account ALIAS]
```

QUERY is a text string. Searches subject and body. Returns same format as `mail list`.

### Read a message

```
yacli mail read UID [--folder FOLDER] [--max-bytes N] [--account ALIAS]
```

UID comes from `mail list` or `mail search` output. Returns full message with `body`, `from`, `to`, `subject`, `date`, `attachments`.

### Send a message

```
yacli mail send TO SUBJECT [BODY] [--cc EMAIL]... [--bcc EMAIL]... [--html HTML] [--account ALIAS]
```

BODY is plain text. Use `--html` for HTML content.

### Reply to a message

```
yacli mail reply UID [BODY] [--folder FOLDER] [--cc EMAIL]... [--html HTML] [--account ALIAS]
```

Replies to the message identified by UID. Preserves threading headers.

### Forward a message

```
yacli mail forward UID TO [BODY] [--folder FOLDER] [--cc EMAIL]... [--bcc EMAIL]... [--html HTML] [--account ALIAS]
```

Forwards the message identified by UID including original content.

## Typical flow

1. `yacli mail list` — see recent messages, note UIDs
2. `yacli mail read UID` — read a specific message
3. `yacli mail reply UID "response text"` — reply
4. `yacli mail search "keyword"` — find messages by text

Always use the `uid` field from list/search output as the ID argument for read/reply/forward.
