mod init;
mod utils;
mod run;
use std::path::PathBuf;
use clap::{Parser, ArgGroup};
use std::str::FromStr;

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[clap(group(
    ArgGroup::new("mode")
        .required(true)
        .args(&["init", "run"])
))]
struct Cli {
    /// Initialize nodes
    #[clap(long)]
    init: bool,

    /// Start the consensus network
    #[clap(long)]
    run: bool,

    /// Path of configuration file
    #[clap(long, parse(from_os_str), value_name = "FILE")]
    config: Option<PathBuf>,
}

fn main() {
    let mut cli = Cli::parse();
    if let None = cli.config {
        cli.config = Some(PathBuf::from_str("config.toml").unwrap());
    }
    if cli.init {
        let ni = init::NodeInitializer::new_with_cfg_file(cli.config.unwrap().as_path());
        ni.do_init_node();
    } else if cli.run {
        
    }
}