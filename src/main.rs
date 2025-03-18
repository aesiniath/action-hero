use anyhow::Result;
use clap::{Arg, ArgAction, Command};
use reqwest;
use std::collections::HashMap;

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
                Arg::new("workflow")
                    .long("workflow")
                    .action(ArgAction::Set)
                    .default_value("check.yaml")
                    .help("Name of the GitHub Actions workflow to present as a trace. The default workflow used if unspecified is check.yaml"))
            .get_matches();

    let workflow = matches.get_one::<String>("workflow").unwrap().to_string();

    println!("{}", workflow);

    hello().await?;

    Ok(())
}
