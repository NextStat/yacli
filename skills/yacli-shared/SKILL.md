---
name: yacli-shared
description: "Yandex services CLI: account management, authentication, and output configuration. Read this skill before using yacli-mail, yacli-calendar, or yacli-disk."
metadata:
  author: NextStat
---

# yacli — shared foundation

yacli is a CLI for Yandex Mail, Calendar, and Disk. All commands return JSON by default.

## Output format

Every command produces `{"ok": true, ...}` on success or `{"ok": false, "code": "...", "message": "..."}` on failure.

Pass `--format table` for human-readable output. Default is `--format json`.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | User error (validation, auth, not found) |
| 4 | Configuration or I/O error |
| 5 | Network or API error |

## Account management

yacli supports multiple accounts. Each account has a short alias derived from the email.

| Command | Description |
|---------|-------------|
| `yacli add <EMAIL> [ALIAS]` | Add account |
| `yacli accounts` | List all accounts |
| `yacli use <ALIAS>` | Switch current account |
| `yacli whoami` | Show current account |
| `yacli status [--account ALIAS]` | Show connected services |

## Authentication

Each service authenticates independently:
- **Mail**: OAuth XOAUTH2 (default) or app password
- **Calendar**: App password only
- **Disk**: OAuth only

| Command | Description |
|---------|-------------|
| `yacli login [SERVICE]` | Connect mail, calendar, or disk |
| `yacli login mail --app-password PASSWORD` | Use app password for mail |
| `yacli login calendar --app-password PASSWORD` | Use app password for calendar |
| `yacli logout [SERVICE] [--account ALIAS]` | Disconnect one or all services |

SERVICE is one of: `mail`, `calendar`, `disk`. Omit to connect all.

## Global flags

- `--format json|table` — output format (default: json)
- `--account ALIAS` — target account (default: current)
- `-h, --help` — show help
- `-V, --version` — show version

## Multi-account usage

Always pass `--account ALIAS` when working with a non-current account:

```
yacli mail list --account work
yacli calendar events --account personal
```
