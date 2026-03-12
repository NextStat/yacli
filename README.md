# yacli

`yacli` — CLI для Яндекс Почты, Календаря и Диска, рассчитанный на людей и AI-агентов.

Важно:

- `yacli` не является официальным продуктом Яндекса
- `yacli` не поддерживается и не сопровождается Яндексом
- `yacli` использует публично доступные пользовательские интерфейсы и API Яндекса, но является независимым open-source проектом `NextStat`

## Статус

Сейчас в репозитории уже доступны следующие стабильные срезы:

- управление именованными аккаунтами для Яндекс Почты, Календаря и Диска
- `account use/current` для активного аккаунта по умолчанию
- детерминированная проверка `auth status` для настроенных аккаунтов
- живые `OAuth`-команды `login/logout` для приватного Яндекс Диска и Почты через `authorization_code + PKCE`
- живое чтение метаданных публичных ресурсов Яндекс Диска
- живая загрузка файлов из публичных ресурсов Яндекс Диска
- живое чтение приватной информации о Диске по сохраненному `OAuth`-токену
- живой browse приватного Яндекс Диска через `disk list`
- живое создание папок в приватном Яндекс Диске через `disk mkdir`
- живая загрузка локальных файлов в приватный Яндекс Диск через `disk upload`
- живое чтение списка почтовых папок через `IMAP + XOAUTH2`
- живой список писем по папке через `IMAP + XOAUTH2`
- живое чтение одного письма по `UID` через `IMAP + MIME parsing`
- живая пересылка писем через inline-forward поверх `IMAP + SMTP + XOAUTH2`
- живая отправка писем через `SMTP + XOAUTH2`
- живое чтение списка календарей через `CalDAV + app password`
- живой список событий календаря через `CalDAV REPORT`
- живое создание событий календаря через `CalDAV PUT`
- живое удаление событий календаря через `CalDAV DELETE`
- сохранение calendar app password прямо через CLI без обязательного `export`
- agent-friendly каталог команд и workflow через `yacli guide`
- `JSON`-ориентированный контракт вывода

Текущий продуктовый фокус:

- только пользовательская поверхность
- только доступ пользователей и агентов к Почте, Календарю и Диску
- без административных и организационных сценариев, доменной админки и `admin API Yandex 360` на текущем этапе

Пока не являются стабильными в `v0.1.17`:

- install/release surface для macOS, Windows и Linux

Эти поверхности намеренно не выводятся как готовые, пока для них нет настоящих адаптеров и живой верификации.

## Установка

```bash
cargo install --path .
```

## Быстрый старт

```bash
yacli account add personal me@yandex.ru --use

yacli account list
yacli account current
yacli guide --topic mail
yacli guide --topic calendar
yacli account validate
yacli auth status

yacli auth login \
  --service disk \
  --client-id <client-id>

yacli auth login \
  --service mail \
  --client-id <client-id>

yacli auth login \
  --service calendar \
  --app-password <app-password>

# Для автоматизации можно передать код подтверждения явно:
yacli auth login \
  --service disk \
  --client-id <client-id> \
  --code <confirmation-code>

yacli disk info
yacli disk list --path disk:/ --limit 50
yacli disk mkdir --path disk:/docs/archive
yacli disk upload --source ./report.pdf --path disk:/docs/archive/report.pdf
yacli mail folders
yacli mail list --folder INBOX --limit 10
yacli mail search --folder INBOX --query "Budget" --limit 5
yacli mail read --folder INBOX --uid 1353
yacli mail reply --folder INBOX --uid 1353 --text "Принято, спасибо"
yacli mail forward --folder INBOX --uid 1353 --to person@example.com --text "FYI"
yacli mail send \
  --to person@example.com \
  --subject "Синк" \
  --text "Привет из yacli"
yacli calendar calendars
yacli calendar events \
  --calendar default \
  --from 2026-03-12 \
  --to 2026-03-19 \
  --limit 20

yacli calendar create \
  --calendar default \
  --summary "Синк команды" \
  --start 2026-03-12T09:00:00Z \
  --end 2026-03-12T10:00:00Z \
  --description "Еженедельный апдейт" \
  --location Meet

yacli calendar delete \
  --calendar default \
  --uid <uid>

yacli auth logout --service disk

yacli disk public show \
  --public-key https://disk.yandex.ru/i/WhGpLnQWR9efCA

yacli disk public download \
  --public-key https://disk.yandex.ru/i/WhGpLnQWR9efCA \
  --output ./sample.pdf
```

## Несколько аккаунтов

`yacli` поддерживает несколько независимых аккаунтов через именованные `account` и переключение текущего контекста.

Пример:

```bash
yacli account add personal me@yandex.ru --use
yacli auth login --service mail --client-id <client-id>

yacli account add work me@company.ru
yacli account use work
yacli auth login --service mail --client-id <client-id>

yacli account use personal
yacli mail folders

yacli account use work
yacli mail search --folder INBOX --query "invoice" --limit 5
yacli mail list --folder INBOX --limit 10
yacli mail reply --folder INBOX --uid 42 --text "Подтверждаю"
yacli mail forward --folder INBOX --uid 42 --to audit@example.com --text "FYI"
```

Стабильный контракт:

- токены хранятся раздельно по `account`
- `auth logout --account <name> --service mail` удаляет токен только выбранного аккаунта
- команды Почты, Диска, Календаря и `auth status` используют текущий аккаунт автоматически
- `--account <name>` остается точечным override для multi-account сценариев и агентов

## Конфигурация

По умолчанию `yacli` хранит конфигурацию здесь:

- macOS: `~/Library/Application Support/yacli/accounts.toml`
- Linux: `~/.config/yacli/accounts.toml`
- Windows: `%APPDATA%\\yacli\\accounts.toml`

Сохраненные `OAuth`-учетные данные и app passwords лежат здесь:

- macOS: `~/Library/Application Support/yacli/credentials.toml`
- Linux: `~/.config/yacli/credentials.toml`
- Windows: `%APPDATA%\\yacli\\credentials.toml`

Для тестов и автоматизации можно переопределить каталог:

```bash
export YACLI_CONFIG_DIR=/path/to/config-dir
```

## Как получить пароль приложения для Календаря

`yacli` для календаря использует не `client_secret` от OAuth-приложения, а отдельный **пароль приложения Яндекс ID** для `CalDAV`.

Шаги:

1. Открой [Пароли приложений в Яндекс ID](https://yandex.ru/support/id/ru/authorization/app-passwords).
2. Перейди в `Безопасность` → `Доступ к вашим данным` → `Пароли приложений`.
3. Выбери тип `Календарь`.
4. Назови пароль, например `yacli calendar`.
5. Нажми `Далее`.
6. Скопируй пароль сразу. Яндекс показывает его только один раз.

Важно:

- это **не** `client_secret` от OAuth-приложения
- для доменной почты email аккаунта должен быть полным: `user@domain`
- по документации Яндекса пароль приложения может начать работать не мгновенно, а в течение `2–3 часов`

## Как правильно ввести пароль в CLI

Рекомендуемый пользовательский путь:

```bash
yacli auth login \
  --service calendar \
  --app-password <app-password>
```

Что делает эта команда:

- сохраняет пароль приложения в `credentials.toml`
- пишет в аккаунт `calendar.credential_ref = "store:calendar"`
- после этого `calendar calendars` и `calendar events` больше не требуют `export`

Проверка:

```bash
yacli auth status
yacli calendar calendars
yacli calendar events --calendar default --from 2026-03-12 --to 2026-03-19 --limit 20
```

Для automation и CI по-прежнему поддержан env-path:

```bash
export YACLI_CALENDAR_APP_PASSWORD='<app-password>'

yacli auth login \
  --service calendar \
  --env-var YACLI_CALENDAR_APP_PASSWORD
```

Этот режим полезен, если ты не хочешь сохранять пароль приложения локально в `credentials.toml`.

## Бар качества

Каждый атомарный срез обязан проходить:

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Стабильная поверхность

Стабильно в `v0.1.17`:

- `yacli guide`
- `yacli account add`
- `yacli account list`
- `yacli account show`
- `yacli account validate`
- `yacli account use`
- `yacli account current`
- `yacli auth login --service mail`
- `yacli auth login --service calendar`
- `yacli auth login --service disk`
- `yacli auth logout --service mail`
- `yacli auth logout --service calendar`
- `yacli auth logout --service disk`
- `yacli auth status`
- `yacli mail folders`
- `yacli mail list`
- `yacli mail search`
- `yacli mail read`
- `yacli mail reply`
- `yacli mail forward`
- `yacli mail send`
- `yacli calendar calendars`
- `yacli calendar events`
- `yacli calendar create`
- `yacli calendar delete`
- `yacli disk info`
- `yacli disk list`
- `yacli disk mkdir`
- `yacli disk upload`
- `yacli disk public show`
- `yacli disk public download`

Эти команды:

- через `yacli guide` отдают машинно-читаемый каталог стабильных команд и workflow
- используют `authorization_code + PKCE` через страницу подтверждения `verification_code`
- сохраняют `OAuth`-токены Почты и Диска по аккаунтам в `credentials.toml`
- умеют сохранять calendar app password по аккаунтам в `credentials.toml`
- не требуют общего `client_secret` для публичного CLI
- используют `accounts.toml` и текущий аккаунт по умолчанию вместо обязательного `--account` в каждой команде
- при истечении токена требуют явный повторный `auth login`
- для Почты используют `IMAP` на `imap.yandex.com:993` и `AUTHENTICATE XOAUTH2`
- для Календаря используют `CalDAV` на `https://caldav.yandex.ru`
- для Календаря поддерживают два login-path:
  - `--app-password <value>` c сохранением в `store:calendar`
  - `--env-var NAME` для automation без локального сохранения секрета
- для `calendar calendars` возвращают `id`, `name`, `href`, `description`
- для `calendar events` принимают `--calendar`, `--from`, `--to`, `--limit`
- для `calendar events` валидируют диапазон `1..=100` и окно дат `from < to`
- для `calendar events` возвращают `uid`, `summary`, `start`, `end`, `location`, `status`, `all_day`
- для `calendar create` принимают `--calendar`, `--summary`, `--start`, `--end`
- для `calendar create` поддерживают timed `RFC3339` и all-day `YYYY-MM-DD`, но оба края должны быть одного формата
- для `calendar create` возвращают созданный `uid`, `href`, `etag`, `summary`, `start`, `end`
- для `calendar delete` принимают `--calendar` и `--uid`, находят событие через `REPORT` и удаляют его через `DELETE`
- для `disk list` принимают `--path`, `--limit`, `--offset` и читают приватный ресурс через `GET /v1/disk/resources`
- для `disk list` по умолчанию используют `disk:/` как корень и возвращают метаданные ресурса плюс `children`, если это папка
- для `disk mkdir` принимают `--path`, создают директорию через `PUT /v1/disk/resources` и затем читают канонические метаданные созданной папки
- для `disk upload` принимают `--source`, `--path`, опциональный `--overwrite`
- для `disk upload` получают upload ticket через `GET /v1/disk/resources/upload`, отправляют бинарный поток файла на provider `href` и затем читают канонические метаданные загруженного ресурса
- для `disk upload` валидируют, что `--source` существует, указывает на файл и не пуст
- для `disk upload` возвращают `source_path`, `remote_path`, `bytes_written`, `sha256`, `overwrite`
- для `mail list` принимают `--folder`, по умолчанию используют `INBOX`
- для `mail list` принимают `--limit`, валидируют диапазон `1..=100`
- для `mail search` принимают `--query`, `--folder`, `--limit` и ищут письма по тексту через IMAP `UID SEARCH`
- для `mail search` поддерживают UTF-8 запросы и возвращают тот же summary contract, что и `mail list`
- для почтовых summary возвращают `uid`, `subject`, `from`, `date`, `flags`, `size`
- для `mail read` принимают `--uid` и читают одно письмо из выбранной папки
- для `mail read` принимают `--max-bytes`, по умолчанию `15728640`
- для `mail read` возвращают `subject`, `from`, `to`, `cc`, `date`, `message_id`, `text_body`, `html_body`, `attachments`
- для `mail reply` принимают `--uid`, `--text`/`--html`, опциональный `--cc` и отвечают в thread через `In-Reply-To` и `References`
- для `mail reply` выбирают адрес ответа из `Reply-To`, а если его нет, используют `From`
- для `mail forward` принимают `--uid`, повторяемые `--to`, опциональные `--cc`, `--bcc`, `--text`, `--html`
- для `mail forward` читают исходное письмо через IMAP, собирают inline-forward тело и отправляют его через текущий SMTP path
- для `mail forward` не пересылают бинарные вложения, а явно перечисляют их как `omitted_attachments` и добавляют notice в тело письма
- для `mail forward` принимают `--max-source-bytes`, по умолчанию `15728640`, и не начинают пересылку при нулевом значении
- для `mail send` используют `SMTP` на `smtp.yandex.com:465`
- для `mail send` принимают повторяемые `--to`, `--cc`, `--bcc`
- для `mail send` требуют `--subject` и хотя бы один из `--text`/`--html`
- для `mail send` возвращают `from`, `to`, `cc`, `bcc_count`, `subject`, `message_id`, `body_kind`
- поддерживают несколько независимых аккаунтов через разные `account` и `account use`

`yacli guide`:

- принимает `--topic all|account|auth|mail|calendar|disk`
- возвращает список стабильных команд с summary и examples
- возвращает типовые workflow, чтобы агенту не приходилось угадывать следующий шаг
- принимают `public key` или публичный URL Яндекс Диска
- опционально принимают `--path` для вложенного ресурса внутри опубликованной папки
- умеют работать без локального аккаунта, используя официальный `REST API` Яндекс Диска
- при наличии профиля могут брать базовый URL Диска из его конфигурации

Дополнительно `yacli disk public download`:

- требует явный `--output`
- отказывается перезаписывать существующий файл без `--force`
- пишет файл атомарно через временный файл и `rename`
- сверяет размер загруженного файла с метаданными провайдера, если размер известен
