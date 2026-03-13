# yacli

`yacli` — утилита командной строки для Яндекс Почты, Календаря и Диска, рассчитанная и на людей, и на AI-агентов.

С ее помощью можно:

- читать, искать, отправлять, пересылать и отвечать на письма;
- смотреть календари и события, создавать и удалять встречи;
- просматривать приватный Диск, создавать папки и загружать файлы;
- работать с несколькими учетными записями и быстро переключаться между ними.

Важно:

- `yacli` не является продуктом Яндекса;
- Яндекс этот проект не поддерживает;
- это самостоятельный проект с открытым исходным кодом.

## Установка

### macOS и Linux

```bash
curl -fsSL https://raw.githubusercontent.com/NextStat/yacli/main/scripts/install.sh | sh
```

### Windows

```powershell
irm https://raw.githubusercontent.com/NextStat/yacli/main/scripts/install.ps1 | iex
```

### Готовые архивы

Готовые сборки лежат в [GitHub Releases](https://github.com/NextStat/yacli/releases).

Публикуемые архивы:

- macOS `x86_64` и `arm64`;
- Linux `x86_64` и `arm64`;
- Windows `x86_64`.

### Из исходников

Этот вариант нужен только тем, кто хочет собирать `yacli` самостоятельно:

```bash
cargo install --path .
```

### Обновление

Если `yacli` уже установлен из release-бинарника или в доступный для записи bin-dir, обновить его можно прямо из CLI:

```bash
yacli update
yacli update --check
```

Если нужен private release mirror или локальный test feed, можно переопределить base URL:

```bash
export YACLI_UPDATE_BASE_URL='https://mirror.example.test/releases/download/v0.2.0'
yacli update --check
```

## Начало работы

### 1. Добавьте адрес

```bash
yacli add me@yandex.ru
yacli whoami
```

Если псевдоним не указать, `yacli` создаст его сам.
Если нужен свой псевдоним:

```bash
yacli add me@yandex.ru personal
```

### 2. Подключите Почту и Диск

```bash
yacli login
```

Эта команда подключает сразу две службы:

- Почту;
- приватный Диск.

Если нужно подключить только одну:

```bash
yacli login mail
yacli login disk
```

### 3. Подключите Календарь

Для Календаря нужен пароль приложения Яндекс ID:

```bash
yacli login calendar --app-password <пароль>
```

### 4. Проверьте, что все работает

```bash
yacli status
yacli mail list
yacli calendar calendars
yacli disk info
```

## Повседневные команды

### Почта

```bash
yacli mail folders
yacli mail list
yacli mail search "смета"
yacli mail read 1353
yacli mail reply 1353 "Принято, спасибо"
yacli mail forward 1353 person@example.com "Посмотрите, пожалуйста"
yacli mail send person@example.com "Синк" "Привет"
```

Что важно:

- `list`, `search`, `read`, `reply` и `forward` по умолчанию работают с папкой `INBOX`;
- число вроде `1353` — это идентификатор письма из вывода `mail list` или `mail search`;
- если нужна другая папка, добавьте `--folder "Имя папки"`;
- если нужен HTML, копии или скрытые копии, используйте `--html`, `--cc` и `--bcc`.

Примеры:

```bash
yacli mail list --folder "Отправленные" --limit 20
yacli mail search "договор" --folder "Архив 2026"
yacli mail send person@example.com "Счет" "Отправляю счет" --cc boss@example.com
```

### Календарь

```bash
yacli calendar calendars

yacli calendar events
yacli calendar events 2026-03-13 2026-03-20 --limit 20

yacli calendar create "Синк команды" 2026-03-13T09:00:00Z 2026-03-13T10:00:00Z

yacli calendar delete <id>
```

Что важно:

- `calendar events` без дат показывает ближайшие 30 дней;
- `calendar create` и `calendar delete` по умолчанию работают с календарем `default`;
- если нужен другой календарь, добавьте `--calendar <id>`.

### Диск

```bash
yacli disk info
yacli disk list
yacli disk list disk:/docs --limit 50
yacli disk mkdir disk:/docs/archive
yacli disk upload ./report.pdf disk:/docs/archive/report.pdf
```

Что важно:

- `disk list` без пути показывает корень `disk:/`;
- `disk mkdir` принимает только путь папки;
- `disk upload` принимает два позиционных аргумента: локальный файл и путь на Диске.

### Публичный Диск

```bash
yacli disk public show \
  --public-key https://disk.yandex.ru/i/WhGpLnQWR9efCA

yacli disk public download \
  --public-key https://disk.yandex.ru/i/WhGpLnQWR9efCA \
  --output ./sample.pdf
```

## Несколько адресов

```bash
yacli add personal@yandex.ru
yacli login

yacli add work@company.ru work
yacli use work
yacli login

yacli use personal
yacli mail list

yacli use work
yacli mail search "счет"
```

Что важно:

- у каждого адреса свои токены и свои настройки;
- команда `use` переключает текущую учетную запись;
- при необходимости можно явно указать `--account <псевдоним>`.

## Как получить пароль приложения для Календаря

1. Откройте [страницу паролей приложений Яндекс ID](https://yandex.ru/support/id/ru/authorization/app-passwords).
2. Перейдите в раздел `Безопасность` → `Доступ к вашим данным` → `Пароли приложений`.
3. Выберите тип `Календарь`.
4. Задайте имя, например `yacli calendar`.
5. Скопируйте пароль сразу после создания.

Подключение:

```bash
yacli login calendar --app-password <пароль>
```

Если не хотите хранить пароль локально:

```bash
export YACLI_CALENDAR_APP_PASSWORD='<пароль>'
yacli login calendar --env-var YACLI_CALENDAR_APP_PASSWORD
```

## Для автоматизации

По умолчанию `yacli` печатает JSON. Если нужен табличный вывод:

```bash
yacli --format table mail list
```

Скрытая команда `guide` предназначена для автоматизации и агентских обвязок:

```bash
yacli guide
yacli guide --topic mail
```

## MCP

`yacli` можно запускать как MCP сервер по `stdio`:

```bash
yacli mcp
```

Если нужен локальный HTTP transport для Apps-capable клиентов:

```bash
yacli mcp --transport http --listen 127.0.0.1:8787
```

Если сервер публикуется за прокси или через внешний URL, укажите канонический адрес:

```bash
yacli mcp --transport http --listen 127.0.0.1:8787 --public-url https://mcp.example.test/mcp
```

Этот HTTP transport теперь streamable:

- `POST /mcp` обрабатывает JSON-RPC requests и batch payloads;
- `GET /mcp` с `Accept: text/event-stream` и `Mcp-Session-Id` открывает SSE stream для server-initiated notifications;
- `DELETE /mcp` завершает HTTP MCP session.

Если нужно защитить HTTP transport bearer-токеном:

```bash
export YACLI_MCP_HTTP_BEARER_TOKEN='secret-token'
yacli mcp --transport http --listen 127.0.0.1:8787
```

Если нужен полноценный auth discovery surface с Protected Resource Metadata и `resource_metadata` в challenge, добавьте issuer:

```bash
export YACLI_MCP_HTTP_AUTH_ISSUER='https://auth.example.test'
```

Чтобы автоматически зарегистрировать сервер в локально доступных MCP-клиентах:

```bash
yacli mcp install
```

Эта команда не только регистрирует MCP сервер, но и раскладывает встроенные agent skills в клиентские каталоги там, где клиент это поддерживает. Сейчас в бинарь встроены:

- `yacli-shared`
- `yacli-mail`
- `yacli-calendar`
- `yacli-disk`
- `yacli-daily-briefing`
- `yacli-find-and-read`
- `yacli-reply-with-context`

Поддерживаемые клиенты:

- Claude Code
- Codex
- Gemini CLI
- Cursor
- Zed
- Windsurf
- Antigravity
- Warp

Если нужен только один клиент:

```bash
yacli mcp install --client codex
yacli mcp install --client cursor
```

Если нужен native HTTP registration в клиентах, которые его документируют:

```bash
yacli mcp install --client claude --transport http --url http://127.0.0.1:8787/mcp
yacli mcp install --client codex --transport http --url http://127.0.0.1:8787/mcp
yacli mcp install --client gemini --transport http --url http://127.0.0.1:8787/mcp
```

Кроме обычных `tools/*` и `resources/read`, сервер также поддерживает resource templates:

```bash
# список шаблонов ресурсов
resources/templates/list

# примеры URI, которые можно читать через resources/read
resource://yacli/account/personal
resource://yacli/auth/personal
ui://yacli/dashboard?account=personal
ui://yacli/dashboard?account=personal&section=auth&resource=auth&tool=yacli.auth.status
```

И `resources/subscribe` / `resources/unsubscribe` для account/auth resources. В `stdio` и streamable `HTTP` это даёт live notifications через `notifications/resources/updated`.

Важно:

- `Claude Code`, `Codex` и `Gemini CLI` умеют native HTTP registration, поэтому `yacli` поддерживает и `stdio`, и `http` install flow;
- для `Cursor`, `Zed`, `Windsurf`, `Warp` и `Antigravity` current stable install path в `yacli` остаётся `stdio`-ориентированным;
- skills автоматически устанавливаются для `Claude Code`, `Codex`, `Gemini CLI`, `Cursor`, `Windsurf`, `Warp` и `Antigravity`; для `Zed` MCP registration поддерживается, но отдельного skills surface сейчас нет;
- сервер может работать и как `stdio`, и как локальный HTTP transport на `/mcp`;
- HTTP transport поддерживает session-scoped SSE stream для server-push notifications;
- если задан `YACLI_MCP_HTTP_BEARER_TOKEN`, защищённые HTTP tool calls требуют `Authorization: Bearer <token>`;
- `YACLI_MCP_HTTP_AUTH_ISSUER` опционален и нужен только если вы хотите включить Protected Resource Metadata и `resource_metadata` в `WWW-Authenticate` challenge;
- `yacli mcp install` не копирует секреты в клиентские конфиги;
- MCP Apps поддерживается с первого релиза через ресурс `ui://yacli/dashboard`, deep-link template `ui://yacli/dashboard{?account,section,resource,tool}`, app-only tool `yacli.app.snapshot`, read-only tool `yacli.update.check` и встроенный resource inspector для templated resources;
- dashboard теперь сам поддерживает round-trip deep links: по мере смены account/resource/tool он пересобирает канонический `ui://yacli/dashboard?...` current view URI и может шарить его обратно в host;
- dashboard также сохраняет последнее локальное view state в браузерном storage и восстанавливает его при следующем открытии, если новый URI не переопределяет эти поля явно;
- dashboard также показывает auth escalation surface: auth discovery, resource metadata и host actions для recovery у protected tools;
- dashboard также умеет безопасно проверять наличие нового release из Apps runtime через `Check updates`, не пытаясь self-replace живой MCP server process;
- обычные текстовые MCP-клиенты продолжают работать без UI.

## Где лежат настройки

- macOS: `~/Library/Application Support/yacli`
- Linux: `~/.config/yacli`
- Windows: `%APPDATA%\\yacli`

Чтобы использовать другой каталог:

```bash
export YACLI_CONFIG_DIR=/path/to/config-dir
```

## Что еще полезно знать

- `status` показывает, какие службы подключены у текущей учетной записи;
- если OAuth-токен Почты или Диска истек, достаточно снова выполнить `yacli login`;
- `mail forward` пересылает письмо вместе с обычными вложениями и inline-файлами;
- `disk public download` не перезаписывает существующий файл без `--force`.

## Проверка качества

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
