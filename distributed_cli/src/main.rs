use clap::Parser;
use distributed_cli::DistributedArgs;

/// The `distributed` CLI: contracts lifecycle, scaffold, client compile,
/// describe manifests, and render schema artifacts.
#[derive(Parser, Debug)]
#[command(name = "distributed", version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    args: DistributedArgs,
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = distributed_cli::run_distributed(&cli.args) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
