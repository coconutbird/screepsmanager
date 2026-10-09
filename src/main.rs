//! `screepsmanager`: uploads built Screeps code to the branches of Screeps
//! servers. Run `screepsmanager help` for usage.
//!
//! The configuration ([`config`]) names servers and profiles: a profile is
//! a server, a branch, and where the branch runs. `upload` reads the
//! modules of the build directory ([`modules`]) and replaces the code of
//! the branch of each selected profile through the Screeps API ([`api`]).

mod api;
mod branch;
mod config;
mod modules;
mod upload;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::config::{Config, ProfileName, ServerName};

/// Uploads built Screeps code to the branches of Screeps servers.
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
    /// The configuration file [default: the nearest screepsmanager.toml in
    /// the working directory or a parent]
    #[arg(
        long,
        short,
        global = true,
        env = "SCREEPSMANAGER_CONFIG",
        value_name = "FILE"
    )]
    config: Option<PathBuf>,
    /// The command.
    #[command(subcommand)]
    command: Command,
}

/// A command.
#[derive(Debug, Subcommand)]
enum Command {
    /// Upload the modules of a build directory to the branches of profiles.
    ///
    /// Each .js file in the directory is a module of its name without .js,
    /// each .js.map file a source map module of its whole name (a JSON map
    /// is wrapped as `module.exports = MAP;`), and each .wasm file a binary
    /// module of its name without .wasm. Other entries are skipped. The
    /// upload replaces every module of each branch, and creates a branch
    /// that the account does not have.
    Upload(upload::Upload),
    /// List the profiles of the configuration.
    Profiles,
}

/// A command that failed.
#[derive(Debug, thiserror::Error)]
enum Error {
    /// The configuration is not available or not valid.
    #[error(transparent)]
    Config(#[from] config::Error),
    /// The build directory does not read as modules.
    #[error(transparent)]
    Modules(#[from] modules::Error),
    /// `auto` names no branch.
    #[error("branch auto: {0}")]
    Git(#[from] branch::GitError),
    /// A secret of a server is not available.
    #[error("server {server}: {source}")]
    Secret {
        /// The server.
        server: ServerName,
        /// Why the secret is not available.
        source: config::SecretError,
    },
    /// A call of the API for a profile failed.
    #[error("profile {profile}: {source}")]
    Api {
        /// The profile.
        profile: ProfileName,
        /// The call that failed.
        source: api::Error,
    },
    /// The output is not writable.
    #[error("the output: {0}")]
    Output(#[from] std::io::Error),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // reqwest uses the process-wide rustls provider; an error means that
    // one is installed already.
    let _ = rustls::crypto::ring::default_provider().install_default();
    match run(cli, &mut std::io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}: {error}", env!("CARGO_PKG_NAME"));
            ExitCode::FAILURE
        }
    }
}

/// Runs the command of `cli` and prints its output to `out`.
fn run(cli: Cli, out: &mut dyn Write) -> Result<(), Error> {
    let config = Config::load(cli.config.as_deref())?;
    writeln!(out, "config {}", config.path.display())?;
    match cli.command {
        Command::Upload(upload) => upload.run(&config, out),
        Command::Profiles => profiles(&config, out),
    }
}

/// Prints the profiles of `config` as a table.
fn profiles(config: &Config, out: &mut dyn Write) -> Result<(), Error> {
    let mut rows =
        vec![["PROFILE", "SERVER", "URL", "BRANCH", "ACTIVATE", "AUTH"].map(String::from)];
    for target in config.profiles() {
        let mark = if config.is_default(target.name) {
            " *"
        } else {
            ""
        };
        let activate: Vec<String> = target
            .profile
            .activate
            .iter()
            .map(ToString::to_string)
            .collect();
        rows.push([
            format!("{}{mark}", target.name),
            target.profile.server.to_string(),
            target.server.url.to_string(),
            target.profile.branch.to_string(),
            if activate.is_empty() {
                "-".to_owned()
            } else {
                activate.join(",")
            },
            target.server.auth.to_string(),
        ]);
    }
    let mut widths = [0; 6];
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    for row in &rows {
        let last = row.len() - 1;
        for (at, (cell, width)) in row.iter().zip(widths).enumerate() {
            if at == last {
                writeln!(out, "{cell}")?;
            } else {
                write!(out, "{cell:<width$}  ")?;
            }
        }
    }
    writeln!(out, "* the profiles of `upload` without --profile")?;
    Ok(())
}
