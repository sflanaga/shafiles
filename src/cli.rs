use anyhow::{anyhow, Context};
use structopt::StructOpt;
use std::path::PathBuf;
use std::time::Duration;
use lazy_static::lazy_static;
use structopt::clap::AppSettings::*;

lazy_static!{
    pub static ref BUILD_INFO: String  = format!("ver: {}  rev: {}  date: {}", env!("CARGO_PKG_VERSION"), env!("VERGEN_SHA_SHORT"), env!("VERGEN_BUILD_DATE"));
}

//structopt::clap::AppSettings::UnifiedHelpMessage

// #[structopt(
// version = BUILD_INFO.as_str(),
// rename_all = "kebab-case",
// )]

use std::env;
type Result<T> = std::result::Result<T, anyhow::Error>;

#[derive(StructOpt, Debug, Clone)]
#[structopt(
version = BUILD_INFO.as_str(), rename_all = "kebab-case",
global_settings(& [
    ColoredHelp,
    ArgRequiredElseHelp
    ]),
)]
///
/// Scans files for changes and log change types against prior runs
///
/// The state (if kept around) will be used as a reference to detect how files changed. Either
/// the content (sha1 hash is tracked) or file last modification timestamp will be detected.
/// If no changes to a particular file are found, then nothing is written.
/// If a file changes (content or timestamp), then it is logged and updated in the state file, but
/// also a count is kept for each file as to changes seen.
///
/// Of course, the first run will log not changes.  It is later runs using an existing state file will
/// that do that.
/// Deleted files will not log a warning... yet.
pub struct Cli {

    #[structopt(short="t", long)]
    /// top of the tree to scan
    pub top_dir: PathBuf,

    #[structopt(short="d", long)]
    /// Number of directory scanning threads
    ///
    /// These threads find the files to perform sha1 on
    pub threads_dir: usize,

    #[structopt(short="s", long)]
    /// Number of sha1 threads
    ///
    /// These threads find the files to perform sha1 on
    pub threads_sha: usize,

    #[structopt(short = "v", parse(from_occurrences))]
    /// log level - e.g. -vvv is the same as debug while -vv is info level
    ///
    /// To true debug your settings you might try trace level or -vvvv
    pub verbosity: usize,

    #[structopt(short="p", long)]
    /// state file path
    pub state_path: PathBuf,

}

pub fn get_cli() -> Cli {
    let mut cli = Cli::from_args();
    if cli.verbosity == 0 {
        cli.verbosity = 2;
    }
    cli
}