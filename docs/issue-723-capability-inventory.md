# Проверяемый каталог возможностей для Issue #723

Срез исходников ветки `feature/full-yandex-mail-disk-723` на 2026-11-24. Это аудит кода, не подтверждение развертывания или доступа к аккаунтам. Полный текст Issue #723 и актуальный `docs/task-rules.md` прочитаны через авторизованный GitHub CLI. Issue требует отдельного покрытия аккаунтов `dedts`, `ykkareliadom`, `dvabobra2014`; реальное состояние ChatGPT и аккаунтов из этого дерева не подтверждено.

| Область | Реализовано в коде | Опубликовано как DevCat MCP | Проверено на аккаунтах | Зависимость / пробел |
|---|---|---|---|---|
| Почта: папки, список, поиск, чтение | Да (`src/mcp/server.rs`, `src/mail.rs`); для списка и поиска есть `offset` до 1 000 000 | `mail.folders/list/search/read` | Нет подтверждения в этой задаче | OAuth Mail; живые запросы по каждому аккаунту не проведены |
| Почта: отправка, ответ, пересылка, вложения | Отправка/ответ/пересылка, экспорт вложений; нативная отправка файлов поддержана в коде | `mail.send/reply/forward/attachment.export` | Нет подтверждения в этой задаче | `mail.send` не включен по умолчанию; сквозной нативный файл + Sent проверяются отдельно; любые отправки должны проходить review gate |
| Почта: создание папок и перенос/копирование UID | Да; отдельные IMAP CREATE/COPY/MOVE реализации в `src/mail.rs` | `mail.folder.create/copy/move`, capability `mail.mutate`, одноразовый review gate | Только протокольные тесты, не аккаунты | Папки корзины/спама можно указать как назначения; отдельные инструменты и правила не реализованы |
| Почта: прочитано/избранное | Да; IMAP `UID STORE` для `\\Seen` и `\\Flagged` | `mail.flags`, capability `mail.mutate`, одноразовый review gate | Только unit и schema тесты | Отдельные пользовательские метки (labels) не реализованы; нет canary на тестовом сообщении |
| Почта: черновики и правила | Создание неотправленного черновика реализовано через IMAP APPEND в объявленную `\\Drafts` папку; обновление/правила не реализованы | `mail.draft.create`, capability `mail.mutate`, review gate | Только unit/protocol/schema tests | Нет отдельной имитации provider Drafts folder lookup и живого аккаунтного canary; правила и delayed sending отсутствуют |
| Почта: нативные файлы, MIME и пересылка | Локальные функции чтения/экспорта и native attachment send/reply/forward есть в `src/mail.rs` | `mail.send/reply/forward/attachment.export` | Нет сквозного canary на созданных тестовых объектах для этой задачи | Требуется завершить/согласовать интеграцию #690 через отдельный проверенный PR; `/home/codex/yandex-mcp-551` не менялось |
| Контакты CardDAV | vCard, поиск/импорт/экспорт и условные create/update/delete реализованы | `contacts.list/search/export/import/create/update/delete`; write закрыт `contacts.write` и review gate | Только unit/MCP тесты; live CardDAV не запускался | На каждом аккаунте нужна отдельная включенная секция и CardDAV app-password key reference; новый пароль приложения должен выдать владелец |
| Диск: сведения, список, чтение | Да (`src/disk.rs`) | `disk.info/list/read` | Нет подтверждения | OAuth Disk; чтение MCP ограничено 1 MiB согласно `docs/devcat-operator.md` |
| Диск: папка, загрузка, скачивание, публикация | Да | `disk.mkdir/upload/download/publish/unpublish/upload_link` | Нет подтверждения | Write/publish scopes; публикация выключена без `disk.publish`; операции записи требуют review |
| Диск: права, восстановление, крупные файлы | Права доступны только через текущую публичную публикацию; отдельные права и восстановление не реализованы. Частные read/download/upload имеют ограничения по размеру | `disk.publish/unpublish` требует отсутствующую в production capability; больших потоковых MCP transfers нет | Нет | Нужны granular permissions/trash API и ограниченная потоковая передача с проверкой review |
| Аккаунты dedts / ykkareliadom / dvabobra2014 | Конфигурация поддерживает точный allowlist аккаунтов | Только конфигурируемые аккаунты | Сводка #642 подтверждает Mail canary ранее; #551 подтверждает базовые read-only Mail/Disk для DevCat. Приёмка полной матрицы #723 не выполнена | Нужны текущие account entries, точные OAuth scopes и отдельные ключи; новый OAuth ykkareliadom — owner-only |
| ChatGPT и Secure MCP Tunnel | Есть конфигурационные примеры и DevCat режим | У loopback production `initialize` и `tools/list` успешны: опубликовано 14 инструментов старой версии, без `mail.mutate`, Contacts и Disk download/publish | Это не проверка каталога ChatGPT через туннель | Нужна приемка каталога ChatGPT, проверка Secure MCP Tunnel и установка нового релиза штатным процессом |
| Hosted Yandex 360 MCP (резерв только для dedts) | Подготовленный безопасный путь: два отдельных HTTP stream endpoint, `Authorization: OAuth <секрет из защищенной настройки>`; токены не записываются в репозиторий или сообщения | Не подключен в ChatGPT | OAuth и `tools/list` не выполнялись | Mail: `https://mcp.360.yandex.net/v1/mail/mcp`; Disk: `https://mcp.360.yandex.net/v1/disk/mcp`; нужны OAuth владельца для dedts и успешный tools/list; см. operator guide |

## Зависимые задачи

- #551 задаёт границы DevCat, least privilege, review gates, central vault и Secure MCP Tunnel; upstream write запрещён.
- #690 требует нативные файлы ChatGPT с проверкой байтов перед SMTP и без повторной отправки. В отдельном дереве `/home/codex/yandex-mcp-551` есть активная незавершённая работа OCR; это дерево не менялось и не интегрировалось.
- Закрытая #642 подтверждает базовую эксплуатацию и сохранение Sent на трёх аккаунтах, но её завершение не доказывает приёмку полного функционала #723.
- Официальная справка Yandex ID перечисляет отдельный пароль приложения для CardDAV; OAuth scope приложения нельзя считать разрешением на контакты. Ссылки: [синхронизация контактов CardDAV](https://www.yandex.ru/support/yandex-360/customers/mail/ru/mail-clients/others), [пароли приложений](https://www.yandex.ru/support/id/ru/authorization/app-passwords.html).

## Read-only production preflight (2026-11-24)

Проверены только account metadata и состояние процесса; секретные значения не читались, пользовательские письма/файлы/контакты не запрашивались.

| Account | Mail metadata | Disk metadata | Contacts metadata | Что доказано |
|---|---|---|---|---|
| `dedts` | enabled, `oauth_xoauth2`, credential reference указан | enabled, `oauth`, credential reference указан | секции нет, значит новый default `enabled=false` | конфигурация службы, но не наличие/валидность OAuth token и не успешный provider read |
| `ykkareliadom` | enabled, `oauth_xoauth2`, credential reference указан | disabled | секции нет, значит новый default `enabled=false` | OAuth Disk требуется отдельная авторизация владельца, если включать целевой Disk |
| `dvabobra2014` | enabled, `oauth_xoauth2`, credential reference указан | disabled | секции нет, значит новый default `enabled=false` | Диск не включен |

`yacli-devcat.service` был `active`, listener — `127.0.0.1:8797`, `GET /healthz` вернул HTTP 200. Это не проверяет Secure MCP Tunnel readiness, ChatGPT tool catalog или account credentials.

Read-only production MCP canary, 2026-11-24: a fresh loopback MCP session returned successful `initialize` and `tools/list` with 14 published tools. Read-only `yacli.mail.folders` returned 64 folders for `dedts` (7.9 s) and 24 for `dvabobra2014` (6.4 s). `ykkareliadom` returned JSON-RPC error `-32000` in 6.0 s; the response did not establish whether the cause was account authorization, provider behavior, or another service error. No folder names or message contents were retained in the audit output. This confirms folder-list access only for `dedts` and `dvabobra2014`; it does not verify ChatGPT through the managed tunnel or any message/Disk operation.

## Основание

- Реестр инструментов и диспетчер: `src/mcp/server.rs`.
- Фильтрация возможностей и review gate: `src/mcp/devcat.rs`.
- Реализация почты и Диска: `src/mail.rs`, `src/disk.rs`.
- Эксплуатационные ограничения OAuth, Secure MCP Tunnel и canary: `docs/devcat-operator.md`.
- `tests/mcp_server.rs` содержит тесты локального реестра инструментов, но они не доказывают живое подключение ChatGPT или реальные операции аккаунтов.

## Безопасная граница

Не выполнять изменения существующих пользовательских сообщений или файлов. Реальные мутации можно проверять только на специально созданных тестовых объектах, с review gate и без внешних получателей. Не расширять права аккаунтов без подтвержденного согласия владельца. Не считать работы по OCR в `/home/codex/yandex-mcp-551` или коммит `2ff12edc` завершенными и не изменять то дерево.

## Официальный резервный hosted MCP маршрут

Официальная документация Яндекс 360 подтверждает Mail endpoint `https://mcp.360.yandex.net/v1/mail/mcp` и Disk endpoint `https://mcp.360.yandex.net/v1/disk/mcp`; оба используют потоковый HTTP MCP и заголовок `Authorization: OAuth <OAuth_токен>`. Это отдельный резерв только для `dedts`: сначала владелец должен пройти OAuth именно в `dedts` и сохранить каждый токен непосредственно в одобренном защищенном интерфейсе ChatGPT, не отправляя его в чат, терминал, issue или логи. В настройках создаются две раздельные записи с соответствующими URL и скрытым секретным заголовком. После OAuth надо выполнить `tools/list` для обоих серверов и безопасные read-only canary; до этого соединение не считается активным. Официальные серверы помечены как предварительные. Mail не отправляет письма (сохраняет черновик для ручной отправки); Disk публикует и меняет только публичные права, а не персональные права пользователей. Доступность hosted сервиса требует подходящего личного или корпоративного тарифа.

Источники: [официальная справка MCP Яндекс Почты](https://yandex.ru/dev/yandex-360/mcp/ru/mail), [официальная справка MCP Яндекс Диска](https://yandex.ru/dev/yandex-360/mcp/ru/disk), [обзор условий доступа Яндекс 360 MCP](https://yandex.ru/dev/yandex-360/mcp/ru/). Токены не запрашивались и не получались; hosted серверы не подключались.
