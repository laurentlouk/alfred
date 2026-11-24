//! `alfred` opens the assistant; `alfred <command>` runs it headless (see `alfred help`).

mod cli;
mod speech;
mod ui;
mod wiring;
mod workers;

const COMMANDS: &[&str] = &["ask", "say", "listen", "setup", "doctor", "eval", "shortcuts", "help", "--help", "-h", "version", "--version"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| COMMANDS.contains(&a.as_str())) {
        std::process::exit(cli::run(&args));
    }
    ui::run(args);
}
