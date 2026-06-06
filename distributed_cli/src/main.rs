use clap::Parser;
use distributed_cli::ServiceArgs;

/// The `dsvc` CLI: scaffold Distributed services, describe their manifest, and
/// render schema artifacts (SQL or Atlas Operator resources).
#[derive(Parser, Debug)]
#[command(name = "dsvc", version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    args: ServiceArgs,
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = distributed_cli::run(&cli.args) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
