use anyhow::Result;
use clap::{Arg, ArgAction, Command};
use reqwest;
use std::collections::HashMap;
use tracing::{debug, info};
use tracing_subscriber;

async fn hello() -> Result<()> {
    let resp = reqwest::get("https://httpbin.org/ip")
        .await?
        .json::<HashMap<String, String>>()
        .await?;
    println!("{resp:#?}");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

    // Initialize the tracing subscriber
    tracing_subscriber::fmt::init();

    let matches = Command::new("hero")
            .version(VERSION)
            .propagate_version(true)
            .author("Andrew Cowie")
            .about("Retrieve workflow and run from GitHub Actions and send to OpenTelemetry as spans and traces.")
            .disable_help_subcommand(true)
            .disable_help_flag(true)
            .disable_version_flag(true)
            .arg(
                Arg::new("help")
                    .long("help")
                    .long_help("Print help")
                    .global(true)
                    .hide(true)
                    .action(ArgAction::Help))
            .arg(
                Arg::new("version")
                    .long("version")
                    .long_help("Print version")
                    .global(true)
                    .hide(true)
                    .action(ArgAction::Version))
            .arg(
                Arg::new("repository")
                    .action(ArgAction::Set)
                    .required(true)
                    .help("Name of the GitHub repository to retrieve workflows from. This must be specified in the form \"owner/repo\""))
            .arg(
                Arg::new("workflow")
                    .action(ArgAction::Set)
                    .required(true)
                    .help("Name of the GitHub Actions workflow to present as a trace. This is typically a filename such as \"check.yaml\""))
            .get_matches();

    let repository = matches
        .get_one::<String>("repository")
        .unwrap()
        .to_string();

    debug!(repository);

    let workflow = matches
        .get_one::<String>("workflow")
        .unwrap()
        .to_string();

    debug!(workflow);

    hello().await?;

    Ok(())
}
