# yacli

`yacli` — утилита командной строки для Яндекс Почты, Календаря и Диска.

С ее помощью можно:

- читать, искать, отправлять, пересылать и отвечать на письма;
- смотреть календари и события, создавать и удалять встречи;
- просматривать приватный Диск, создавать папки и загружать файлы;
- работать с несколькими учетными записями и быстро переключаться между ними.

Важно:

- `yacli` не является продуктом Яндекса;
- Яндекс этот проект не поддерживает;
- это самостоятельный open-source проект.

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

### Из исходников

Этот путь нужен только тем, кто хочет собирать `yacli` самостоятельно:

```bash
cargo install --path .
```

## Начало работы

### 1. Добавьте учетную запись

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
  --id <id>
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

## Несколько учетных записей

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

- у каждой учетной записи свои токены и свои настройки;
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
