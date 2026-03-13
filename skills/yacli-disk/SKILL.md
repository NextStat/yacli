---
name: yacli-disk
description: "Yandex Disk: list files, create folders, upload and download via REST API. Use when the task involves file storage."
metadata:
  author: NextStat
---

# yacli disk

Read `yacli-shared` first for account and auth setup.

## Commands

### Disk info

```
yacli disk info [--account ALIAS]
```

Returns quota: total, used, and available space.

### List files

```
yacli disk list [PATH] [--limit N] [--offset N] [--account ALIAS]
```

PATH uses `disk:/` prefix. Defaults: path=disk:/, limit=100, offset=0.

Example:
```
yacli disk list disk:/Documents --limit 50
```

### Create a folder

```
yacli disk mkdir PATH [--account ALIAS]
```

Example:
```
yacli disk mkdir disk:/Documents/reports
```

### Upload a file

```
yacli disk upload FILE PATH [--overwrite] [--account ALIAS]
```

FILE is a local path. PATH is the remote destination.

Example:
```
yacli disk upload ./report.pdf disk:/Documents/report.pdf --overwrite
```

### Public files

```
yacli disk public show --public-key URL [--path PATH] [--account ALIAS]
yacli disk public download --public-key URL --output FILE [--path PATH] [--force] [--account ALIAS]
```

Access shared files by their public link.

## Typical flow

1. `yacli disk info` — check available space
2. `yacli disk list disk:/path` — browse files
3. `yacli disk upload ./file disk:/path/file` — upload a file
