use clap::{Parser, Subcommand, ValueEnum};
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

#[derive(Debug, Parser)]
#[command(
    name = "yacli",
    version,
    about = "Командная строка для Яндекс Почты, Календаря и Диска"
)]
pub struct Cli {
    #[arg(long, value_enum, global = true, default_value_t = OutputFormat::Json)]
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
        #[arg(value_name = "ALIAS")]
        name: Option<String>,
    },
    /// Показать все настроенные аккаунты.
    Accounts,
    /// Сделать аккаунт текущим.
    Use {
        #[arg(value_name = "ALIAS")]
        name: String,
    },
    /// Показать текущий аккаунт.
    Whoami,
    /// Показать, что подключено у аккаунта.
    Status {
        #[arg(long)]
        account: Option<String>,
    },
    /// Подключить Почту, Диск или Календарь.
    Login {
        service: Option<AuthServiceArg>,
        #[arg(long)]
        account: Option<String>,
        #[arg(long, hide = true)]
        client_id: Option<String>,
        #[arg(long)]
        env_var: Option<String>,
        #[arg(long)]
        app_password: Option<String>,
        #[arg(long, hide = true)]
        code: Option<String>,
        #[arg(long, hide = true)]
        login_hint: Option<String>,
    },
    /// Отключить один сервис или все сервисы у аккаунта.
    Logout {
        service: Option<AuthServiceArg>,
        #[arg(long)]
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
        #[arg(long)]
        account: Option<String>,
    },
    Validate {
        #[arg(long)]
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
        #[arg(long)]
        account: Option<String>,
    },
    Login {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, value_enum)]
        service: Option<AuthServiceArg>,
        #[arg(long, hide = true)]
        client_id: Option<String>,
        #[arg(long)]
        env_var: Option<String>,
        #[arg(long)]
        app_password: Option<String>,
        #[arg(long, hide = true)]
        code: Option<String>,
        #[arg(long, hide = true)]
        login_hint: Option<String>,
    },
    Logout {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, value_enum)]
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
        #[arg(long)]
        account: Option<String>,
        #[arg(value_name = "PATH")]
        path: String,
    },
    /// Загрузить локальный файл в приватный Диск.
    Upload {
        #[arg(long)]
        account: Option<String>,
        #[arg(value_name = "SOURCE")]
        source: PathBuf,
        #[arg(value_name = "PATH")]
        path: String,
        #[arg(long, default_value_t = false)]
        overwrite: bool,
    },
    /// Показать содержимое папки в приватном Диске.
    List {
        #[arg(long)]
        account: Option<String>,
        #[arg(value_name = "PATH")]
        path: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: u64,
    },
    /// Показать квоту и общую информацию о приватном Диске.
    Info {
        #[arg(long)]
        account: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum DiskPublicCommand {
    /// Показать информацию о публичном файле или папке.
    Show {
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        public_key: String,
        #[arg(long)]
        path: Option<String>,
    },
    /// Скачать публичный файл Яндекс Диска.
    Download {
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        public_key: String,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum MailCommand {
    /// Показать папки в почтовом ящике.
    Folders {
        #[arg(long)]
        account: Option<String>,
    },
    /// Показать список писем. По умолчанию используется папка INBOX.
    List {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "INBOX")]
        folder: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Найти письма по тексту. По умолчанию поиск идет в INBOX.
    Search {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "INBOX")]
        folder: String,
        #[arg(value_name = "TEXT")]
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Ответить на письмо по ID из `mail list` или `mail search`.
    Reply {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "INBOX")]
        folder: String,
        #[arg(value_name = "ID")]
        uid: u64,
        #[arg(value_name = "TEXT")]
        body: Option<String>,
        #[arg(long)]
        cc: Vec<String>,
        #[arg(long)]
        html: Option<String>,
    },
    /// Переслать письмо по ID из `mail list` или `mail search`.
    Forward {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "INBOX")]
        folder: String,
        #[arg(value_name = "ID")]
        uid: u64,
        #[arg(value_name = "TO")]
        to: String,
        #[arg(value_name = "TEXT")]
        body: Option<String>,
        #[arg(long)]
        cc: Vec<String>,
        #[arg(long)]
        bcc: Vec<String>,
        #[arg(long)]
        html: Option<String>,
        #[arg(long, default_value_t = 15 * 1024 * 1024)]
        max_source_bytes: u64,
    },
    /// Открыть письмо по ID из `mail list` или `mail search`.
    Read {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "INBOX")]
        folder: String,
        #[arg(value_name = "ID")]
        uid: u64,
        #[arg(long, default_value_t = 15 * 1024 * 1024)]
        max_bytes: u64,
    },
    /// Отправить новое письмо.
    Send {
        #[arg(long)]
        account: Option<String>,
        #[arg(value_name = "TO")]
        to: String,
        #[arg(value_name = "SUBJECT")]
        subject: String,
        #[arg(value_name = "TEXT")]
        body: Option<String>,
        #[arg(long)]
        cc: Vec<String>,
        #[arg(long)]
        bcc: Vec<String>,
        #[arg(long)]
        html: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum CalendarCommand {
    /// Показать доступные календари.
    Calendars {
        #[arg(long)]
        account: Option<String>,
    },
    /// Показать события в окне дат.
    Events {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "default")]
        calendar: String,
        #[arg(value_name = "FROM")]
        from: Option<String>,
        #[arg(value_name = "TO")]
        to: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Создать событие в календаре.
    Create {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "default")]
        calendar: String,
        #[arg(value_name = "SUMMARY")]
        summary: String,
        #[arg(value_name = "START")]
        start: String,
        #[arg(value_name = "END")]
        end: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        location: Option<String>,
    },
    /// Удалить событие по ID.
    Delete {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "default")]
        calendar: String,
        #[arg(value_name = "ID")]
        uid: String,
    },
}
