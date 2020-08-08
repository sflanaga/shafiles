use anyhow::{anyhow, Context};
use structopt::StructOpt;
use std::path::PathBuf;
use std::time::Duration;

type Result<T> = std::result::Result<T, anyhow::Error>;

#[derive(StructOpt, Debug, Clone)]
#[structopt(
rename_all = "kebab-case",
global_settings(& [
structopt::clap::AppSettings::ColoredHelp,
structopt::clap::AppSettings::UnifiedHelpMessage
]),
)]
pub struct Cli {
    #[structopt(short, long)]
    /// top of the tree to scan
    pub top_dir: PathBuf,

    #[structopt(short, long)]
    /// Number of directory scanning threads
    ///
    /// These threads find the files to perform sha1 on
    pub dir_threads: usize,

    #[structopt(short, long)]
    /// Number of sha1 threads
    ///
    /// These threads find the files to perform sha1 on
    pub sha_threads: usize,

    #[structopt(short = "v", parse(from_occurrences))]
    /// log level - e.g. -vvv is the same as debug while -vv is info level
    ///
    /// To true debug your settings you might try trace level or -vvvv
    pub verbosity: usize,
}

pub fn get_cli() -> Cli {
    let mut cli = Cli::from_args();
    if cli.verbosity == 0 {
        cli.verbosity = 2;
    }
    cli
}