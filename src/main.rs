#![forbid(unsafe_code)]

mod cli;
mod git;
mod json;
mod report;
mod run;
mod scan;

use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

use crate::cli::Cli;

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    let root = match &cli.root {
        Some(root) => root.clone(),
        None => std::env::current_dir().context("could not determine current directory")?,
    };
    let code = if cli.recursive {
        run::multi(&root, &cli)?
    } else {
        run::single(&root, &cli)?
    };

    Ok(ExitCode::from(code))
}
