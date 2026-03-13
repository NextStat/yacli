---
name: yacli-mail
description: "Яндекс Почта: поиск, чтение, отправка, ответ и пересылка писем через IMAP/SMTP. Используй для любых задач с почтой — прочитать письмо, отправить email, найти сообщение, ответить, переслать."
metadata:
  author: NextStat
---

# yacli mail — Яндекс Почта

Сначала прочитай `yacli-shared` для настройки аккаунта и авторизации.

## Команды

### Список папок

```
yacli mail folders [--account ALIAS]
```

### Список писем

```
yacli mail list [--folder FOLDER] [--limit N] [--account ALIAS]
```

По умолчанию: folder=INBOX, limit=20. Возвращает массив с `uid`, `from`, `subject`, `date`.

### Поиск писем

```
yacli mail search ЗАПРОС [--folder FOLDER] [--limit N] [--account ALIAS]
```

ЗАПРОС — текстовая строка. Ищет по теме и телу письма. Формат как у `mail list`.

### Чтение письма

```
yacli mail read UID [--folder FOLDER] [--max-bytes N] [--account ALIAS]
```

UID берётся из вывода `mail list` или `mail search`. Возвращает полное письмо: `body`, `from`, `to`, `subject`, `date`, `attachments`.

### Отправка письма

```
yacli mail send КОМУ ТЕМА [ТЕКСТ] [--cc EMAIL]... [--bcc EMAIL]... [--html HTML] [--account ALIAS]
```

ТЕКСТ — простой текст. `--html` для HTML-содержимого.

### Ответ на письмо

```
yacli mail reply UID [ТЕКСТ] [--folder FOLDER] [--cc EMAIL]... [--html HTML] [--account ALIAS]
```

Отвечает на письмо по UID. Сохраняет заголовки цепочки.

### Пересылка письма

```
yacli mail forward UID КОМУ [ТЕКСТ] [--folder FOLDER] [--cc EMAIL]... [--bcc EMAIL]... [--html HTML] [--account ALIAS]
```

Пересылает письмо с оригинальным содержимым.

## Типичный порядок работы

1. `yacli mail list` — просмотреть входящие, запомнить UID
2. `yacli mail read UID` — прочитать письмо
3. `yacli mail reply UID "текст ответа"` — ответить
4. `yacli mail search "ключевое слово"` — найти письма по тексту

UID из вывода list/search — обязательный аргумент для read/reply/forward.
