use clap::{Arg, ArgAction, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    Json,
    Table,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum MailAuthModeArg {
    OauthXoauth2,
    AppPassword,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum CalendarAuthModeArg {
    AppPassword,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum DiskAuthModeArg {
    Oauth,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum AuthServiceArg {
    Mail,
    Calendar,
    Disk,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum GuideTopicArg {
    All,
    Account,
    Auth,
    Mail,
    Calendar,
    Disk,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum McpClientArg {
    Claude,
    ClaudeDesktop,
    Codex,
    Gemini,
    Warp,
    Zed,
    Cursor,
    Antigravity,
    Windsurf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum McpTransportArg {
    Stdio,
    Http,
}

impl McpTransportArg {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
        }
    }
}

const HELP_TEMPLATE: &str = "\
{before-help}{about-with-newline}\
Использование:\n    {usage}\n\
\n\
{all-args}{after-help}\
";

#[derive(Debug, Parser)]
#[command(
    name = "yacli",
    version,
    about = "Командная строка для Яндекс Почты, Календаря и Диска"
)]
pub struct Cli {
    #[arg(
        long,
        value_enum,
        global = true,
        default_value_t = OutputFormat::Json,
        value_name = "ФОРМАТ",
        help = "Формат вывода"
    )]
    pub format: OutputFormat,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(hide = true)]
    Guide {
        #[arg(long, value_enum, default_value_t = GuideTopicArg::All)]
        topic: GuideTopicArg,
    },
    /// Добавить аккаунт. Если псевдоним не указан, он будет создан из email.
    Add {
        #[arg(value_name = "EMAIL")]
        email: String,
        #[arg(value_name = "ПСЕВДОНИМ")]
        name: Option<String>,
    },
    /// Провести первичную настройку аккаунта и MCP за один проход.
    Setup {
        #[arg(value_name = "EMAIL")]
        email: Option<String>,
        #[arg(long, value_name = "ПСЕВДОНИМ")]
        name: Option<String>,
        #[arg(long, value_name = "ПАРОЛЬ")]
        calendar_app_password: Option<String>,
        #[arg(long, value_name = "ПЕРЕМЕННАЯ")]
        calendar_env_var: Option<String>,
        #[arg(long = "client", value_enum, value_name = "КЛИЕНТ")]
        client: Vec<McpClientArg>,
        #[arg(
            long,
            value_enum,
            default_value_t = McpTransportArg::Stdio,
            value_name = "ТРАНСПОРТ",
            help = "Какой транспорт регистрировать при setup"
        )]
        mcp_transport: McpTransportArg,
        #[arg(
            long,
            value_name = "URL",
            help = "URL MCP HTTP сервера, если выбран HTTP transport"
        )]
        mcp_url: Option<String>,
        #[arg(
            long,
            default_value_t = false,
            help = "Не выполнять OAuth login для Почты и Диска"
        )]
        skip_login: bool,
        #[arg(long, default_value_t = false, help = "Не выполнять `mcp install`")]
        skip_mcp_install: bool,
        #[arg(
            long,
            default_value_t = false,
            help = "Только собрать план onboarding без реальных изменений"
        )]
        plan_only: bool,
    },
    /// Показать единый home screen по аккаунту, onboarding, workflows и activity.
    Home {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, value_name = "ЦЕЛЬ")]
        goal: Option<String>,
    },
    /// Проверить продуктовый health-check: конфиг, секреты, сервисы и readiness workflows.
    Doctor {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, value_name = "ЦЕЛЬ")]
        goal: Option<String>,
        #[arg(
            long,
            default_value_t = false,
            help = "Применить только безопасные локальные remediation-шаги без новых пользовательских данных"
        )]
        apply_safe: bool,
    },
    /// Показать 3-5 следующих действий с наибольшим продуктовым эффектом.
    Next {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, value_name = "ЦЕЛЬ")]
        goal: Option<String>,
    },
    /// Маршрутизировать естественную цель в лучший workflow, prompt и MCP tool-path.
    Goal {
        #[arg(value_name = "ЗАПРОС")]
        query: String,
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
    },
    /// Показать все настроенные аккаунты.
    Accounts,
    /// Показать журнал последних действий и replay-команды.
    Activity {
        #[command(subcommand)]
        action: ActivityCommand,
    },
    /// Показать готовые кросс-сервисные workflow yacli.
    Workflow {
        #[command(subcommand)]
        action: WorkflowCommand,
    },
    /// Сделать аккаунт текущим.
    Use {
        #[arg(value_name = "ALIAS")]
        name: String,
    },
    /// Показать текущий аккаунт.
    Whoami,
    /// Показать, что подключено у аккаунта.
    Status {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
    },
    /// Подключить Почту, Диск или Календарь.
    Login {
        #[arg(value_name = "СЕРВИС")]
        service: Option<AuthServiceArg>,
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, hide = true)]
        client_id: Option<String>,
        #[arg(long, value_name = "ПЕРЕМЕННАЯ")]
        env_var: Option<String>,
        #[arg(long, value_name = "ПАРОЛЬ")]
        app_password: Option<String>,
        #[arg(long, hide = true)]
        code: Option<String>,
        #[arg(long, hide = true)]
        login_hint: Option<String>,
    },
    /// Отключить один сервис или все сервисы у аккаунта.
    Logout {
        #[arg(value_name = "СЕРВИС")]
        service: Option<AuthServiceArg>,
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
    },
    #[command(hide = true)]
    Account {
        #[command(subcommand)]
        action: AccountCommand,
    },
    #[command(hide = true)]
    Auth {
        #[command(subcommand)]
        action: AuthCommand,
    },
    /// Файлы и папки Яндекс Диска.
    Disk {
        #[command(subcommand)]
        action: DiskCommand,
    },
    /// Календари и события Яндекс Календаря.
    Calendar {
        #[command(subcommand)]
        action: CalendarCommand,
    },
    /// Письма и папки Яндекс Почты.
    Mail {
        #[command(subcommand)]
        action: MailCommand,
    },
    /// Запустить MCP сервер по stdio или установить его в поддерживаемые клиенты.
    Mcp {
        #[arg(
            long,
            value_enum,
            default_value_t = McpTransportArg::Stdio,
            value_name = "ТРАНСПОРТ",
            help = "Транспорт MCP сервера"
        )]
        transport: McpTransportArg,
        #[arg(
            long,
            default_value = "127.0.0.1:8787",
            value_name = "АДРЕС",
            help = "Адрес для HTTP транспорта"
        )]
        listen: String,
        #[arg(
            long,
            value_name = "URL",
            help = "Канонический публичный URL MCP сервера для HTTP auth discovery"
        )]
        public_url: Option<String>,
        #[command(subcommand)]
        action: Option<McpCommand>,
    },
    /// Обновить yacli из GitHub Releases.
    Update {
        #[arg(
            long,
            default_value = "latest",
            value_name = "ВЕРСИЯ",
            help = "Версия без префикса v или latest"
        )]
        version: String,
        #[arg(
            long,
            default_value_t = false,
            help = "Только проверить, доступно ли обновление"
        )]
        check: bool,
        #[arg(long, hide = true, value_name = "URL")]
        base_url: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum McpCommand {
    /// Установить yacli MCP сервер в локально доступные клиенты.
    Install {
        #[arg(long = "client", value_enum, value_name = "КЛИЕНТ")]
        client: Vec<McpClientArg>,
        #[arg(
            long,
            value_enum,
            default_value_t = McpTransportArg::Stdio,
            value_name = "ТРАНСПОРТ",
            help = "Какой транспорт регистрировать в клиенте"
        )]
        transport: McpTransportArg,
        #[arg(
            long,
            value_name = "URL",
            help = "URL MCP HTTP сервера, если выбран HTTP транспорт"
        )]
        url: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum WorkflowCommand {
    /// Показать все встроенные workflow.
    List,
    /// Показать один workflow с шагами CLI и MCP.
    Show {
        #[arg(value_name = "WORKFLOW")]
        id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ActivityCommand {
    /// Показать последние действия.
    List {
        #[arg(long, default_value_t = 20, value_name = "ЧИСЛО")]
        limit: usize,
    },
    /// Показать одно действие и replay-команду.
    Show {
        #[arg(value_name = "ID")]
        id: String,
    },
    /// Откатить одно обратимое действие.
    Undo {
        #[arg(value_name = "ID")]
        id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum AccountCommand {
    Add {
        name: String,
        email: String,
        #[arg(long = "use", default_value_t = false)]
        use_as_current: bool,
        #[arg(long, value_enum, default_value_t = MailAuthModeArg::OauthXoauth2)]
        mail_auth_mode: MailAuthModeArg,
        #[arg(long, value_enum, default_value_t = CalendarAuthModeArg::AppPassword)]
        calendar_auth_mode: CalendarAuthModeArg,
        #[arg(long, value_enum, default_value_t = DiskAuthModeArg::Oauth)]
        disk_auth_mode: DiskAuthModeArg,
        #[arg(long)]
        mail_credential_ref: Option<String>,
        #[arg(long)]
        calendar_credential_ref: Option<String>,
        #[arg(long)]
        disk_credential_ref: Option<String>,
    },
    List,
    Show {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
    },
    Validate {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
    },
    Use {
        name: String,
    },
    Current,
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    Status {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
    },
    Login {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, value_enum, value_name = "СЕРВИС")]
        service: Option<AuthServiceArg>,
        #[arg(long, hide = true)]
        client_id: Option<String>,
        #[arg(long, value_name = "ПЕРЕМЕННАЯ")]
        env_var: Option<String>,
        #[arg(long, value_name = "ПАРОЛЬ")]
        app_password: Option<String>,
        #[arg(long, hide = true)]
        code: Option<String>,
        #[arg(long, hide = true)]
        login_hint: Option<String>,
    },
    Logout {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, value_enum, value_name = "СЕРВИС")]
        service: Option<AuthServiceArg>,
    },
}

#[derive(Debug, Subcommand)]
pub enum DiskCommand {
    /// Работать с публичным файлом или папкой Яндекс Диска.
    Public {
        #[command(subcommand)]
        action: DiskPublicCommand,
    },
    /// Создать папку в приватном Диске.
    Mkdir {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(value_name = "ПУТЬ")]
        path: String,
    },
    /// Загрузить локальный файл в приватный Диск.
    Upload {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(value_name = "ФАЙЛ")]
        source: PathBuf,
        #[arg(value_name = "ПУТЬ")]
        path: String,
        #[arg(long, default_value_t = false)]
        overwrite: bool,
        #[arg(
            long,
            default_value_t = false,
            help = "Только проверить и показать, что будет загружено, без реального upload"
        )]
        dry_run: bool,
    },
    /// Загрузить локальный файл на Диск и сразу получить публичную ссылку.
    UploadLink {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, value_name = "ФАЙЛ")]
        source: PathBuf,
        #[arg(long, value_name = "ПУТЬ")]
        path: String,
        #[arg(long, default_value_t = false)]
        overwrite: bool,
        #[arg(
            long,
            default_value_t = false,
            help = "Только проверить и показать, что будет загружено и опубликовано, без реального upload/publish"
        )]
        dry_run: bool,
    },
    /// Скачать файл из приватного Диска.
    Download {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(value_name = "ПУТЬ")]
        path: String,
        #[arg(long, value_name = "ФАЙЛ")]
        output: PathBuf,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Опубликовать приватный файл или папку и получить публичную ссылку.
    Publish {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(value_name = "ПУТЬ")]
        path: String,
        #[arg(
            long,
            default_value_t = false,
            help = "Только проверить и показать, что будет опубликовано, без реального publish"
        )]
        dry_run: bool,
    },
    /// Отозвать публичную ссылку у приватного файла или папки.
    Unpublish {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(value_name = "ПУТЬ")]
        path: String,
        #[arg(
            long,
            default_value_t = false,
            help = "Только проверить и показать, что будет отозвано, без реального unpublish"
        )]
        dry_run: bool,
    },
    /// Показать содержимое папки в приватном Диске.
    List {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(value_name = "ПУТЬ")]
        path: Option<String>,
        #[arg(long, default_value_t = 100, value_name = "ЧИСЛО")]
        limit: usize,
        #[arg(long, default_value_t = 0, value_name = "СМЕЩЕНИЕ")]
        offset: u64,
    },
    /// Показать квоту и общую информацию о приватном Диске.
    Info {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum DiskPublicCommand {
    /// Показать информацию о публичном файле или папке.
    Show {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, value_name = "ССЫЛКА")]
        public_key: String,
        #[arg(long, value_name = "ПУТЬ")]
        path: Option<String>,
    },
    /// Скачать публичный файл Яндекс Диска.
    Download {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, value_name = "ССЫЛКА")]
        public_key: String,
        #[arg(long, value_name = "ПУТЬ")]
        path: Option<String>,
        #[arg(long, value_name = "ФАЙЛ")]
        output: PathBuf,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum MailCommand {
    /// Показать папки в почтовом ящике.
    Folders {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
    },
    /// Показать список писем. По умолчанию используется папка INBOX.
    List {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, default_value = "INBOX", value_name = "ПАПКА")]
        folder: String,
        #[arg(long, default_value_t = 20, value_name = "ЧИСЛО")]
        limit: usize,
    },
    /// Найти письма по тексту. По умолчанию поиск идет в INBOX.
    Search {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, default_value = "INBOX", value_name = "ПАПКА")]
        folder: String,
        #[arg(value_name = "ТЕКСТ")]
        query: String,
        #[arg(long, default_value_t = 20, value_name = "ЧИСЛО")]
        limit: usize,
    },
    /// Ответить на письмо по ID из `mail list` или `mail search`.
    Reply {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, default_value = "INBOX", value_name = "ПАПКА")]
        folder: String,
        #[arg(value_name = "ID")]
        uid: u64,
        #[arg(value_name = "ТЕКСТ")]
        body: Option<String>,
        #[arg(long, value_name = "EMAIL")]
        cc: Vec<String>,
        #[arg(long, value_name = "HTML")]
        html: Option<String>,
    },
    /// Переслать письмо по ID из `mail list` или `mail search`.
    Forward {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, default_value = "INBOX", value_name = "ПАПКА")]
        folder: String,
        #[arg(value_name = "ID")]
        uid: u64,
        #[arg(value_name = "EMAIL")]
        to: String,
        #[arg(value_name = "ТЕКСТ")]
        body: Option<String>,
        #[arg(long, value_name = "EMAIL")]
        cc: Vec<String>,
        #[arg(long, value_name = "EMAIL")]
        bcc: Vec<String>,
        #[arg(long, value_name = "HTML")]
        html: Option<String>,
        #[arg(long, default_value_t = 15 * 1024 * 1024, value_name = "БАЙТЫ")]
        max_source_bytes: u64,
    },
    /// Открыть письмо по ID из `mail list` или `mail search`.
    Read {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, default_value = "INBOX", value_name = "ПАПКА")]
        folder: String,
        #[arg(value_name = "ID")]
        uid: u64,
        #[arg(long, default_value_t = 15 * 1024 * 1024, value_name = "БАЙТЫ")]
        max_bytes: u64,
    },
    /// Отправить новое письмо.
    Send {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(value_name = "EMAIL")]
        to: String,
        #[arg(value_name = "ТЕМА")]
        subject: String,
        #[arg(value_name = "ТЕКСТ")]
        body: Option<String>,
        #[arg(long, value_name = "EMAIL")]
        cc: Vec<String>,
        #[arg(long, value_name = "EMAIL")]
        bcc: Vec<String>,
        #[arg(long, value_name = "HTML")]
        html: Option<String>,
        #[arg(long = "attach", value_name = "ФАЙЛ")]
        attachments: Vec<PathBuf>,
        #[arg(
            long,
            default_value_t = false,
            help = "Только проверить и показать, что будет отправлено, без реальной отправки"
        )]
        dry_run: bool,
    },
    /// Загрузить файл на Диск, опубликовать ссылку и отправить её письмом.
    SendLink {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(value_name = "EMAIL")]
        to: String,
        #[arg(value_name = "ТЕМА")]
        subject: String,
        #[arg(value_name = "ТЕКСТ")]
        body: Option<String>,
        #[arg(long, value_name = "EMAIL")]
        cc: Vec<String>,
        #[arg(long, value_name = "EMAIL")]
        bcc: Vec<String>,
        #[arg(long, value_name = "HTML")]
        html: Option<String>,
        #[arg(long, value_name = "ФАЙЛ")]
        source: PathBuf,
        #[arg(long, value_name = "ПУТЬ")]
        path: String,
        #[arg(long, default_value_t = false)]
        overwrite: bool,
        #[arg(
            long,
            default_value_t = false,
            help = "Только проверить upload/publish/send flow без реальных write-действий"
        )]
        dry_run: bool,
    },
    /// Работа с вложениями писем.
    Attachment {
        #[command(subcommand)]
        action: MailAttachmentCommand,
    },
    /// Работа с календарными приглашениями из писем.
    Invite {
        #[command(subcommand)]
        action: MailInviteCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum MailAttachmentCommand {
    /// Сохранить вложение письма в локальный файл.
    Export {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, default_value = "INBOX", value_name = "ПАПКА")]
        folder: String,
        #[arg(value_name = "ID")]
        uid: u64,
        #[arg(long, value_name = "ЧИСЛО")]
        index: Option<usize>,
        #[arg(long, value_name = "ИМЯ")]
        name: Option<String>,
        #[arg(long, value_name = "ФАЙЛ")]
        output: PathBuf,
        #[arg(long, default_value_t = false)]
        force: bool,
        #[arg(long, default_value_t = 15 * 1024 * 1024, value_name = "БАЙТЫ")]
        max_bytes: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum MailInviteCommand {
    /// Разобрать календарное приглашение из вложения письма.
    Inspect {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, default_value = "INBOX", value_name = "ПАПКА")]
        folder: String,
        #[arg(value_name = "ID")]
        uid: u64,
        #[arg(long, value_name = "ЧИСЛО")]
        index: Option<usize>,
        #[arg(long, value_name = "ИМЯ")]
        name: Option<String>,
        #[arg(long, default_value_t = 15 * 1024 * 1024, value_name = "БАЙТЫ")]
        max_bytes: u64,
    },
    /// Создать событие в календаре из приглашения во вложении письма.
    CreateEvent {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, default_value = "INBOX", value_name = "ПАПКА")]
        folder: String,
        #[arg(value_name = "ID")]
        uid: u64,
        #[arg(long, value_name = "ЧИСЛО")]
        index: Option<usize>,
        #[arg(long, value_name = "ИМЯ")]
        name: Option<String>,
        #[arg(long, default_value = "default", value_name = "КАЛЕНДАРЬ")]
        calendar: String,
        #[arg(long, default_value_t = 1, value_name = "ЧИСЛО")]
        event_index: usize,
        #[arg(long, default_value_t = 15 * 1024 * 1024, value_name = "БАЙТЫ")]
        max_bytes: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum CalendarCommand {
    /// Показать доступные календари.
    Calendars {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
    },
    /// Показать события в окне дат.
    Events {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, default_value = "default", value_name = "КАЛЕНДАРЬ")]
        calendar: String,
        #[arg(value_name = "ОТ")]
        from: Option<String>,
        #[arg(value_name = "ДО")]
        to: Option<String>,
        #[arg(long, default_value_t = 20, value_name = "ЧИСЛО")]
        limit: usize,
    },
    /// Создать событие в календаре.
    Create {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, default_value = "default", value_name = "КАЛЕНДАРЬ")]
        calendar: String,
        #[arg(value_name = "НАЗВАНИЕ")]
        summary: String,
        #[arg(value_name = "НАЧАЛО")]
        start: String,
        #[arg(value_name = "КОНЕЦ")]
        end: String,
        #[arg(long, value_name = "ОПИСАНИЕ")]
        description: Option<String>,
        #[arg(long, value_name = "МЕСТО")]
        location: Option<String>,
        #[arg(
            long,
            default_value_t = false,
            help = "Только проверить и показать, что будет создано, без записи в календарь"
        )]
        dry_run: bool,
    },
    /// Удалить событие по ID.
    Delete {
        #[arg(long, value_name = "АККАУНТ")]
        account: Option<String>,
        #[arg(long, default_value = "default", value_name = "КАЛЕНДАРЬ")]
        calendar: String,
        #[arg(value_name = "ID")]
        uid: String,
    },
}

pub fn parse_cli() -> Cli {
    let command = build_cli_command();
    let matches = command.get_matches();
    Cli::from_arg_matches(&matches).unwrap_or_else(|err| err.exit())
}

pub fn build_cli_command() -> clap::Command {
    localize_help(Cli::command(), true)
}

fn localize_help(mut command: clap::Command, is_root: bool) -> clap::Command {
    command = command
        .help_template(HELP_TEMPLATE)
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .subcommand_help_heading("Команды")
        .subcommand_value_name("КОМАНДА")
        .next_help_heading("Параметры")
        .mut_args(|arg| {
            if arg.get_help_heading().is_none() {
                let heading = if arg.is_positional() {
                    "Аргументы"
                } else {
                    "Параметры"
                };
                arg.help_heading(heading)
            } else {
                arg
            }
        });

    command = command.arg(
        Arg::new("help")
            .short('h')
            .long("help")
            .action(ArgAction::Help)
            .help("Показать справку")
            .help_heading("Параметры"),
    );

    if is_root {
        command = command.disable_version_flag(true).arg(
            Arg::new("version")
                .short('V')
                .long("version")
                .action(ArgAction::Version)
                .help("Показать версию")
                .help_heading("Параметры"),
        );
    }

    command.mut_subcommands(|subcommand| localize_help(subcommand, false))
}
