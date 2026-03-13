use serde_json::Value;

use crate::cli::OutputFormat;

pub struct RenderedOutput {
    pub format: OutputFormat,
    pub json: Value,
    pub table: String,
    pub exit_code: i32,
}

pub fn emit(rendered: RenderedOutput) {
    match rendered.format {
        OutputFormat::Json => println!("{}", rendered.json),
        OutputFormat::Table => println!("{}", rendered.table),
    }
}
