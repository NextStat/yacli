# yacli

`yacli` — консольная утилита для работы с Яндекс Почтой, Календарем и Диском.

С ее помощью можно:

- завести несколько аккаунтов и переключаться между ними
- подключить Почту и Диск через OAuth
- подключить Календарь через пароль приложения
- читать письма, отвечать на них, пересылать и отправлять новые
- смотреть календари и события, создавать и удалять события
- просматривать приватный Диск, создавать папки и загружать файлы
- смотреть и скачивать публичные файлы и папки Яндекс Диска

`yacli` подходит и для обычной работы в терминале, и для скриптов: по умолчанию он выводит JSON.

Важно:

- `yacli` не является продуктом Яндекса
- проект не поддерживается Яндексом
- это независимый open-source проект

## Что уже поддерживается

Сейчас в проекте есть рабочие команды для:

- аккаунтов
- Почты
- Календаря
- Диска
- публичного Диска

Готовые пользовательские каналы установки:

- `Homebrew` через tap `NextStat/yacli`
- `winget` через архив с manifest-файлами, публикуемый в каждом релизе
- `install.sh` для macOS и Linux
- `install.ps1` для Windows
- установка из исходников через `cargo install --path .`

## Установка

### Homebrew

Подключить tap:

```bash
brew tap NextStat/yacli https://github.com/NextStat/yacli
```

Установить:

```bash
brew install NextStat/yacli/yacli
```

Что важно:

- tap живет прямо в этом репозитории
- формула ставит готовый бинарь из GitHub Releases
- сейчас Homebrew-пакеты публикуются для `macOS arm64` и `macOS x86_64`
- для Linux пока остаются `install.sh` или `cargo install`

### macOS и Linux

Последний опубликованный релиз:

```bash
curl -fsSL https://raw.githubusercontent.com/NextStat/yacli/main/scripts/install.sh | sh
```

Конкретная версия:

```bash
curl -fsSL https://raw.githubusercontent.com/NextStat/yacli/main/scripts/install.sh | sh -s -- --version 0.1.19
```

По умолчанию бинарь ставится в `~/.local/bin`.

### Windows

Через `winget`:

```powershell
$tmp = Join-Path $env:TEMP "yacli-winget"
Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
Invoke-WebRequest https://github.com/NextStat/yacli/releases/latest/download/winget-manifests.zip -OutFile (Join-Path $tmp "winget-manifests.zip")
Expand-Archive -Path (Join-Path $tmp "winget-manifests.zip") -DestinationPath $tmp -Force
winget settings --enable LocalManifestFiles
winget install --manifest (Join-Path $tmp "NextStat.yacli") --accept-package-agreements --disable-interactivity
```

Конкретная версия:

```powershell
$tmp = Join-Path $env:TEMP "yacli-winget"
Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
Invoke-WebRequest https://github.com/NextStat/yacli/releases/download/v0.1.19/winget-manifests.zip -OutFile (Join-Path $tmp "winget-manifests.zip")
Expand-Archive -Path (Join-Path $tmp "winget-manifests.zip") -DestinationPath $tmp -Force
winget settings --enable LocalManifestFiles
winget install --manifest (Join-Path $tmp "NextStat.yacli") --accept-package-agreements --disable-interactivity
```

Если нужен более короткий путь без ручной распаковки manifest-файлов, используй `install.ps1`.

Последний опубликованный релиз:

```powershell
irm https://raw.githubusercontent.com/NextStat/yacli/main/scripts/install.ps1 | iex
```

Конкретная версия:

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/NextStat/yacli/main/scripts/install.ps1))) -Version 0.1.19
```

По умолчанию бинарь ставится в `%LOCALAPPDATA%\Programs\yacli\bin`.

Текущие release-артефакты собираются для:

- macOS `aarch64`
- macOS `x86_64`
- Linux `x86_64`
- Windows `x86_64`

### Из исходников

```bash
cargo install --path .
```

## С чего начать

Перед работой лучше сначала открыть встроенную справку. Это самый простой способ понять, какие команды уже поддерживаются и как ими пользоваться.

```bash
yacli guide
yacli guide --topic account
yacli guide --topic auth
yacli guide --topic mail
yacli guide --topic calendar
yacli guide --topic disk
```

## Быстрый старт

### 1. Добавить аккаунт

```bash
yacli account add personal me@yandex.ru --use
yacli account current
```

### 2. Подключить Почту

```bash
yacli auth login --service mail --client-id <client-id>
```

### 3. Подключить Календарь

```bash
yacli auth login --service calendar --app-password <app-password>
```

### 4. Подключить приватный Диск

```bash
yacli auth login --service disk --client-id <client-id>
```

### 5. Проверить, что все работает

```bash
yacli auth status

yacli mail folders
yacli mail list --folder INBOX --limit 10
yacli mail read --folder INBOX --uid 1353

yacli calendar calendars
yacli calendar events --calendar default --from 2026-03-12 --to 2026-03-19 --limit 20

yacli disk info
yacli disk list --path disk:/ --limit 50
```

## Основные команды

### Аккаунты

```bash
yacli account add personal me@yandex.ru --use
yacli account list
yacli account show
yacli account validate
yacli account current
yacli account use personal
```

### Почта

```bash
yacli mail folders
yacli mail list --folder INBOX --limit 10
yacli mail search --folder INBOX --query "Budget" --limit 5
yacli mail read --folder INBOX --uid 1353

yacli mail reply \
  --folder INBOX \
  --uid 1353 \
  --text "Принято, спасибо"

yacli mail forward \
  --folder INBOX \
  --uid 1353 \
  --to person@example.com \
  --text "FYI"

yacli mail send \
  --to person@example.com \
  --subject "Синк" \
  --text "Привет из yacli"
```

### Календарь

```bash
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
```

### Приватный Диск

```bash
yacli disk info

yacli disk list \
  --path disk:/ \
  --limit 50

yacli disk mkdir \
  --path disk:/docs/archive

yacli disk upload \
  --source ./report.pdf \
  --path disk:/docs/archive/report.pdf
```

### Публичный Диск

```bash
yacli disk public show \
  --public-key https://disk.yandex.ru/i/WhGpLnQWR9efCA

yacli disk public download \
  --public-key https://disk.yandex.ru/i/WhGpLnQWR9efCA \
  --output ./sample.pdf
```

## Работа с несколькими аккаунтами

У `yacli` можно завести несколько независимых аккаунтов и переключаться между ними.

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
```

Что важно:

- у каждого аккаунта свои учетные данные
- `account use` переключает текущий аккаунт
- если нужно, можно явно указать аккаунт через `--account <name>`
- `auth logout --account <name> --service <service>` удаляет данные только у выбранного аккаунта

## Как устроен вход

### Почта и Диск

Для Почты и Диска используется OAuth с PKCE.

```bash
yacli auth login --service mail --client-id <client-id>
yacli auth login --service disk --client-id <client-id>
```

Если код подтверждения уже известен, его можно передать сразу:

```bash
yacli auth login \
  --service disk \
  --client-id <client-id> \
  --code <confirmation-code>
```

Выход:

```bash
yacli auth logout --service mail
yacli auth logout --service disk
```

### Календарь

Для Календаря используется не OAuth, а пароль приложения Яндекс ID для CalDAV.

Обычный вариант:

```bash
yacli auth login \
  --service calendar \
  --app-password <app-password>
```

После этого пароль приложения сохраняется локально, и команды календаря можно запускать без дополнительных переменных окружения.

Выход:

```bash
yacli auth logout --service calendar
```

### Если не хочешь хранить пароль локально

Можно привязать переменную окружения:

```bash
export YACLI_CALENDAR_APP_PASSWORD='<app-password>'

yacli auth login \
  --service calendar \
  --env-var YACLI_CALENDAR_APP_PASSWORD
```

Этот вариант удобен для автоматических сценариев и CI.

## Как получить пароль приложения для Календаря

1. Открой [Пароли приложений в Яндекс ID](https://yandex.ru/support/id/ru/authorization/app-passwords)
2. Перейди в `Безопасность` → `Доступ к вашим данным` → `Пароли приложений`
3. Выбери тип `Календарь`
4. Задай понятное имя, например `yacli calendar`
5. Скопируй пароль сразу после создания

Важно:

- это не `client_secret` от OAuth-приложения
- для доменной почты нужно указывать полный email, например `user@domain`
- по документации Яндекса пароль приложения может начать работать не сразу, а в течение `2–3 часов`

## Где хранятся файлы настроек

Файл с аккаунтами:

- macOS: `~/Library/Application Support/yacli/accounts.toml`
- Linux: `~/.config/yacli/accounts.toml`
- Windows: `%APPDATA%\\yacli\\accounts.toml`

Файл с учетными данными:

- macOS: `~/Library/Application Support/yacli/credentials.toml`
- Linux: `~/.config/yacli/credentials.toml`
- Windows: `%APPDATA%\\yacli\\credentials.toml`

Если нужно использовать другой каталог:

```bash
export YACLI_CONFIG_DIR=/path/to/config-dir
```

## Формат вывода

По умолчанию `yacli` выводит JSON.

Если нужен табличный вид:

```bash
yacli --format table account list
```

Поддерживаются два варианта:

- `--format json`
- `--format table`

## Поддерживаемые команды

В `v0.1.19` к поддерживаемым относятся:

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

Стабильные каналы установки в `v0.1.19`:

- `Homebrew` tap `NextStat/yacli`
- `winget` архив с manifest-файлами из GitHub Release assets
- `scripts/install.sh` для macOS и Linux
- `scripts/install.ps1` для Windows

Если команды нет в `yacli guide`, лучше не рассчитывать на нее как на часть публичного интерфейса.

## Что полезно знать заранее

- `auth status` показывает, настроен ли доступ к Почте, Календарю и Диску
- если OAuth-токен для Почты или Диска истек, нужно заново выполнить `auth login`
- `mail list` и `mail search` принимают `--limit` от `1` до `100`
- `mail read` ограничивает размер письма через `--max-bytes`
- `mail reply` отвечает на `Reply-To`, а если его нет, то на `From`
- `mail forward` не пересылает бинарные вложения; вместо этого перечисляет их в `omitted_attachments` и добавляет заметку в текст письма
- `mail send` требует `--subject` и хотя бы одно из полей: `--text` или `--html`
- `calendar events` проверяет диапазон дат и значение `--limit`
- `calendar create` поддерживает два формата дат: `RFC3339` для обычных событий и `YYYY-MM-DD` для событий на весь день
- `disk upload` проверяет, что файл существует, не пуст и действительно является файлом
- `disk public download` требует `--output` и не перезаписывает существующий файл без `--force`

## Проверка качества

Для изменений в поддерживаемых командах ожидается, что проходят:

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Лицензия

MIT
