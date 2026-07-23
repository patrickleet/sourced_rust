use clap::Parser;
use distributed_cli::ServiceArgs;

/// The `dctl` CLI: scaffold Distributed services, compile typed client
/// artifacts, describe manifests, and render schema artifacts.
#[derive(Parser, Debug)]
#[command(name = "dctl", version, about, long_about = None)]
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
