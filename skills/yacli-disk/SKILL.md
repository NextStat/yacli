---
name: yacli-disk
description: "Яндекс Диск: файлы, папки, загрузка и скачивание через REST API. Используй для задач с файлами — посмотреть что на диске, загрузить документ, скачать файл, создать папку, публичные ссылки."
metadata:
  author: NextStat
---

# yacli disk — Яндекс Диск

Сначала прочитай `yacli-shared` для настройки аккаунта и авторизации.

## Команды

### Информация о диске

```
yacli disk info [--account ALIAS]
```

Возвращает квоту: общий объём, занято, свободно.

### Список файлов

```
yacli disk list [ПУТЬ] [--limit N] [--offset N] [--account ALIAS]
```

ПУТЬ использует префикс `disk:/`. По умолчанию: path=disk:/, limit=100, offset=0.

Пример:
```
yacli disk list disk:/Документы --limit 50
```

### Создать папку

```
yacli disk mkdir ПУТЬ [--account ALIAS]
```

Пример:
```
yacli disk mkdir disk:/Документы/отчёты
```

### Загрузить файл

```
yacli disk upload ФАЙЛ ПУТЬ [--overwrite] [--account ALIAS]
```

ФАЙЛ — локальный путь. ПУТЬ — место назначения на диске.

Пример:
```
yacli disk upload ./отчёт.pdf disk:/Документы/отчёт.pdf --overwrite
```

### Публичные файлы

```
yacli disk public show --public-key URL [--path ПУТЬ] [--account ALIAS]
yacli disk public download --public-key URL --output ФАЙЛ [--path ПУТЬ] [--force] [--account ALIAS]
```

Доступ к расшаренным файлам по публичной ссылке.

## Типичный порядок работы

1. `yacli disk info` — проверить свободное место
2. `yacli disk list disk:/путь` — просмотреть файлы
3. `yacli disk upload ./файл disk:/путь/файл` — загрузить файл
