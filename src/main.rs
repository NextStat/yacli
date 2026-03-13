use std::process;

use yacli::cli::{Command, McpTransportArg, parse_cli};
use yacli::commands::execute;
use yacli::mcp::server::{serve_http, serve_stdio};
use yacli::output::emit;

fn main() {
    let cli = parse_cli();
    if let Command::Mcp {
        transport,
        listen,
        public_url,
        action: None,
    } = &cli.command
    {
        let result = match transport {
            McpTransportArg::Stdio => serve_stdio(),
            McpTransportArg::Http => serve_http(listen, public_url.as_deref()),
        };
        if let Err(err) = result {
            err.exit();
        }
        return;
    }
    match execute(cli) {
        Ok(rendered) => {
            let exit_code = rendered.exit_code;
            emit(rendered);
            if exit_code != 0 {
                process::exit(exit_code);
            }
        }
        Err(err) => err.exit(),
    }
}
