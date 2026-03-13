---
name: yacli-calendar
description: "Яндекс Календарь: просмотр событий, создание и удаление встреч через CalDAV. Используй для задач с расписанием — что запланировано, когда встреча, создать событие, свободное время."
metadata:
  author: NextStat
---

# yacli calendar — Яндекс Календарь

Сначала прочитай `yacli-shared` для настройки аккаунта и авторизации.

## Команды

### Список календарей

```
yacli calendar calendars [--account ALIAS]
```

Возвращает доступные календари с именами и ID.

### Список событий

```
yacli calendar events [ОТ] [ДО] [--calendar ИМЯ] [--limit N] [--account ALIAS]
```

ОТ и ДО — даты в формате `YYYY-MM-DD`. По умолчанию: 30 дней от сегодня, limit=20.

Пример — события на завтра:
```
yacli calendar events 2026-03-14 2026-03-15
```

Пример — события на следующую неделю:
```
yacli calendar events 2026-03-13 2026-03-20
```

### Создать событие

```
yacli calendar create НАЗВАНИЕ НАЧАЛО КОНЕЦ [--calendar ИМЯ] [--description ТЕКСТ] [--location МЕСТО] [--account ALIAS]
```

НАЧАЛО и КОНЕЦ — ISO 8601: `2026-03-14T10:00:00`.

Пример:
```
yacli calendar create "Синхронизация команды" 2026-03-14T10:00:00 2026-03-14T11:00:00 --description "Еженедельный стендап"
```

### Удалить событие

```
yacli calendar delete UID [--calendar ИМЯ] [--account ALIAS]
```

UID берётся из поля `uid` в выводе `calendar events`.

## Типичный порядок работы

1. `yacli calendar calendars` — посмотреть доступные календари
2. `yacli calendar events` — посмотреть ближайшие события
3. `yacli calendar create ...` — создать событие
