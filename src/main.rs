use yacli::cli::parse_cli;
use yacli::commands::execute;
use yacli::output::emit;

fn main() {
    let cli = parse_cli();
    match execute(cli) {
        Ok(rendered) => emit(rendered),
        Err(err) => err.exit(),
    }
}
