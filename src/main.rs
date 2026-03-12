use clap::Parser;

use yacli::cli::Cli;
use yacli::commands::execute;
use yacli::output::emit;

fn main() {
    let cli = Cli::parse();
    match execute(cli) {
        Ok(rendered) => emit(rendered),
        Err(err) => err.exit(),
    }
}
