# Проверяемый каталог возможностей для Issue #723

Срез исходников ветки `feature/full-yandex-mail-disk-723` на 2026-11-24. Это аудит кода, не подтверждение развертывания или доступа к аккаунтам. Полный текст Issue #723 и актуальный `docs/task-rules.md` прочитаны через авторизованный GitHub CLI. Issue требует отдельного покрытия аккаунтов `dedts`, `ykkareliadom`, `dvabobra2014`; реальное состояние ChatGPT и аккаунтов из этого дерева не подтверждено.

| Область | Реализовано в коде | Опубликовано как DevCat MCP | Проверено на аккаунтах | Зависимость / пробел |
|---|---|---|---|---|
| Почта: папки, список, поиск, чтение | Да (`src/mcp/server.rs`, `src/mail.rs`) | `mail.folders/list/search/read` | Нет подтверждения в этой задаче | OAuth Mail; пагинацию поиска отдельно не подтвердили |
| Почта: отправка, ответ, пересылка, вложения | Отправка/ответ/пересылка, экспорт вложений; нативная отправка файлов поддержана в коде | `mail.send/reply/forward/attachment.export` | Нет подтверждения в этой задаче | `mail.send` не включен по умолчанию; сквозной нативный файл + Sent проверяются отдельно; любые отправки должны проходить review gate |
| Почта: перемещение/копирование/спам/корзина/черновики/правила | Не обнаружено в DevCat tool registry | Нет | Нет | IMAP/API провайдера; изучить поддержку и безопасную модель до реализации |
| Контакты CardDAV | Не обнаружено в DevCat tool registry или сервисном модуле | Нет | Нет | Официальная документация Яндекса указывает `carddav.yandex.ru` и отдельный пароль приложения типа CardDAV; текущий OAuth login `mail`/`disk` не выдаёт CardDAV credential. Нужна безопасная отдельная credential reference и owner authorization |
| Диск: сведения, список, чтение | Да (`src/disk.rs`) | `disk.info/list/read` | Нет подтверждения | OAuth Disk; чтение MCP ограничено 1 MiB согласно `docs/devcat-operator.md` |
| Диск: папка, загрузка, скачивание, публикация | Да | `disk.mkdir/upload/download/publish/unpublish/upload_link` | Нет подтверждения | Write/publish scopes; публикация выключена без `disk.publish`; операции записи требуют review |
| Диск: права, восстановление, крупные файлы | Нет подтвержденных соответствующих MCP операций | Нет | Нет | Проверить REST API; продумать review и потоковую передачу |
| Аккаунты dedts / ykkareliadom / dvabobra2014 | Конфигурация поддерживает точный allowlist аккаунтов | Только конфигурируемые аккаунты | Сводка #642 подтверждает Mail canary ранее; #551 подтверждает базовые read-only Mail/Disk для DevCat. Приёмка полной матрицы #723 не выполнена | Нужны текущие account entries, точные OAuth scopes и отдельные ключи; новый OAuth ykkareliadom — owner-only |
| ChatGPT и Secure MCP Tunnel | Есть конфигурационные примеры и DevCat режим | Живой каталог в ChatGPT не проверен | Нет | Нужны доступ к рабочей среде, состояние туннеля и успешные `tools/list` |
| Hosted Yandex 360 MCP (резерв для dedts) | Маршрут подключения не подтвержден | Нет | Нет | Официальные инструкции требуют отдельной проверки; нельзя передавать токен в чат/лог |

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

## Основание

- Реестр инструментов и диспетчер: `src/mcp/server.rs`.
- Фильтрация возможностей и review gate: `src/mcp/devcat.rs`.
- Реализация почты и Диска: `src/mail.rs`, `src/disk.rs`.
- Эксплуатационные ограничения OAuth, Secure MCP Tunnel и canary: `docs/devcat-operator.md`.
- `tests/mcp_server.rs` содержит тесты локального реестра инструментов, но они не доказывают живое подключение ChatGPT или реальные операции аккаунтов.

## Безопасная граница

Не выполнять изменения существующих пользовательских сообщений или файлов. Реальные мутации можно проверять только на специально созданных тестовых объектах, с review gate и без внешних получателей. Не расширять права аккаунтов без подтвержденного согласия владельца. Не считать работы по OCR в `/home/codex/yandex-mcp-551` или коммит `2ff12edc` завершенными и не изменять то дерево.
