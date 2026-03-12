# yacli

`yacli` — CLI для Яндекс Почты, Календаря и Диска.

С ним можно:

- читать, искать, отправлять, отвечать и пересылать письма
- смотреть календари и события, создавать и удалять встречи
- просматривать приватный Диск, создавать папки и загружать файлы
- скачивать публичные файлы и папки Яндекс Диска
- держать несколько аккаунтов и быстро переключаться между ними

Важно:

- `yacli` не является продуктом Яндекса
- проект не поддерживается Яндексом
- это независимый open-source проект

## Установка

### macOS

Через Homebrew:

```bash
brew tap NextStat/yacli https://github.com/NextStat/yacli
brew install NextStat/yacli/yacli
```

Или installer script:

```bash
curl -fsSL https://raw.githubusercontent.com/NextStat/yacli/main/scripts/install.sh | sh
```

### Linux

```bash
curl -fsSL https://raw.githubusercontent.com/NextStat/yacli/main/scripts/install.sh | sh
```

### Windows

Основной путь:

```powershell
irm https://raw.githubusercontent.com/NextStat/yacli/main/scripts/install.ps1 | iex
```

Дополнительно можно использовать `winget` через manifest bundle из GitHub Release.

### Из исходников

```bash
cargo install --path .
```

## Быстрый старт

### 1. Добавь аккаунт

```bash
yacli add me@yandex.ru
yacli whoami
```

Если имя не указать, `yacli` сам сделает короткий псевдоним из email.  
Если хочешь задать имя сам:

```bash
yacli add me@yandex.ru personal
```

### 2. Подключи Почту и Диск

```bash
yacli login
```

Эта команда использует встроенное OAuth-приложение `yacli` и подключает сразу:

- Яндекс Почту
- приватный Яндекс Диск

Если нужен только один сервис:

```bash
yacli login mail
yacli login disk
```

### 3. Подключи Календарь

```bash
yacli login calendar --app-password <app-password>
```

### 4. Проверь, что все работает

```bash
yacli status

yacli mail folders
yacli mail list --limit 10
yacli calendar calendars
yacli disk info
```

## Ежедневные команды

### Почта

```bash
yacli mail folders
yacli mail list --limit 10
yacli mail search --query "invoice" --limit 10
yacli mail read 1353
yacli mail reply 1353 --text "Принято, спасибо"
yacli mail forward 1353 --to person@example.com --text "FYI"
yacli mail send --to person@example.com --subject "Синк" --text "Привет"
```

Что важно:

- по умолчанию `list`, `search`, `read`, `reply` и `forward` работают с `INBOX`
- число вроде `1353` берется из колонки `UID`, которую показывают `mail list` и `mail search`
- если нужна другая папка, просто добавь `--folder "Имя папки"`

Примеры:

```bash
yacli mail list --folder "Отправленные" --limit 20
yacli mail read 1353 --folder "Архив 2026"
```

### Календарь

```bash
yacli calendar calendars

yacli calendar events \
  --calendar default \
  --from 2026-03-13 \
  --to 2026-03-20 \
  --limit 20

yacli calendar create \
  --calendar default \
  --summary "Синк команды" \
  --start 2026-03-13T09:00:00Z \
  --end 2026-03-13T10:00:00Z

yacli calendar delete \
  --calendar default \
  --uid <uid>
```

### Диск

```bash
yacli disk info
yacli disk list --path disk:/ --limit 50
yacli disk mkdir --path disk:/docs/archive
yacli disk upload --source ./report.pdf --path disk:/docs/archive/report.pdf
```

### Публичный Диск

```bash
yacli disk public show \
  --public-key https://disk.yandex.ru/i/WhGpLnQWR9efCA

yacli disk public download \
  --public-key https://disk.yandex.ru/i/WhGpLnQWR9efCA \
  --output ./sample.pdf
```

## Несколько аккаунтов

```bash
yacli add personal@yandex.ru
yacli login

yacli add work@company.ru work
yacli use work
yacli login

yacli use personal
yacli mail list --limit 5

yacli use work
yacli mail search --query "invoice" --limit 5
```

Что важно:

- у каждого аккаунта свои токены и свои настройки
- `use` переключает текущий аккаунт
- если нужно, можно явно указать `--account <alias>`

## Календарь: как получить пароль приложения

Для Календаря нужен не OAuth-токен, а пароль приложения Яндекс ID.

Как его создать:

1. Открой [Пароли приложений в Яндекс ID](https://yandex.ru/support/id/ru/authorization/app-passwords)
2. Перейди в `Безопасность` → `Доступ к вашим данным` → `Пароли приложений`
3. Выбери тип `Календарь`
4. Задай имя, например `yacli calendar`
5. Скопируй пароль сразу после создания

Подключение:

```bash
yacli login calendar --app-password <app-password>
```

Если не хочешь хранить пароль локально:

```bash
export YACLI_CALENDAR_APP_PASSWORD='<app-password>'
yacli login calendar --env-var YACLI_CALENDAR_APP_PASSWORD
```

## Для скриптов и агентов

По умолчанию `yacli` печатает JSON.  
Если нужен табличный вывод:

```bash
yacli --format table mail list --limit 10
```

Есть два формата:

- `--format json`
- `--format table`

Для машинного discovery есть скрытая команда:

```bash
yacli guide
yacli guide --topic mail
```

Она нужна в первую очередь для агентов и автоматизации, а не для обычного ручного сценария.

## Где лежат настройки

- macOS: `~/Library/Application Support/yacli`
- Linux: `~/.config/yacli`
- Windows: `%APPDATA%\\yacli`

Если нужен другой каталог:

```bash
export YACLI_CONFIG_DIR=/path/to/config-dir
```

## Что полезно знать

- `status` показывает, что подключено у текущего аккаунта
- если OAuth-токен для Почты или Диска истек, просто снова выполни `yacli login`
- `mail forward` пока не пересылает бинарные вложения как настоящие attachments; он перечисляет их в `omitted_attachments`
- `disk public download` не перезаписывает существующий файл без `--force`

## Проверка качества

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Лицензия

MIT
