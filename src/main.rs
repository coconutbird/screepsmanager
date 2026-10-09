//! `screepmanager`: uploads built Screeps code to a branch of a Screeps
//! server. Run `screepmanager help` for usage.
//!
//! `upload DIR --target NAME` reads the modules of the build directory DIR
//! ([`modules`]), takes the destination NAME of the configuration file
//! ([`config`]), and replaces the code of the destination's branch through
//! the Screeps API ([`api`]): it creates the branch when the account does
//! not have it, and with `--activate` makes it the branch that runs.

mod api;
mod config;
mod modules;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command as Process, ExitCode};

use clap::{Parser, Subcommand};

use crate::api::{Active, Client};
use crate::config::AUTO_BRANCH;

/// Uploads built Screeps code to a branch of a Screeps server.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// The command.
    #[command(subcommand)]
    command: Command,
}

/// A command.
#[derive(Debug, Subcommand)]
enum Command {
    /// Upload the modules of a build directory to the branch of a destination.
    ///
    /// Each .js file in DIR is a module of its name without .js, each .js.map
    /// file a source map module of its whole name (a JSON map is wrapped as
    /// `module.exports = MAP;`), and each .wasm file a binary module of its
    /// name without .wasm. Other entries are skipped. The upload replaces
    /// every module of the branch, and creates the branch when it is missing.
    Upload(Upload),
}

/// The arguments of `upload`.
#[derive(Debug, clap::Args)]
struct Upload {
    /// The build directory.
    dir: PathBuf,
    /// The destination: a name in the configuration file.
    #[arg(long, short, value_name = "NAME")]
    target: String,
    /// The configuration file.
    #[arg(
        long,
        short,
        value_name = "FILE",
        default_value = "screeps.config.json"
    )]
    config: PathBuf,
    /// The branch, instead of the one of the destination; auto is the
    /// current git branch.
    #[arg(long, short)]
    branch: Option<String>,
    /// Make the branch the one that runs in the world or in the simulator
    /// (repeatable).
    #[arg(long, value_enum, value_name = "WHERE")]
    activate: Vec<Active>,
    /// Read the modules and the destination, print them, and upload nothing.
    #[arg(long)]
    dry_run: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // reqwest uses the process-wide rustls provider; an error means that
    // one is installed already.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let out = &mut std::io::stdout().lock();
    let result = match cli.command {
        Command::Upload(upload) => upload.run(out),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("screepmanager: {error}");
            ExitCode::FAILURE
        }
    }
}

impl Upload {
    /// Runs the upload and prints each step to `out`.
    fn run(self, out: &mut dyn Write) -> Result<(), String> {
        let destination = config::destination(&self.config, &self.target)?;
        let branch = match self.branch.as_deref().unwrap_or(&destination.branch) {
            AUTO_BRANCH => {
                git_branch().map_err(|error| format!("branch {AUTO_BRANCH}: {error}"))?
            }
            branch => branch.to_owned(),
        };
        let build = modules::read(&self.dir)?;
        let mut emit = |text: &str| writeln!(out, "{text}").map_err(|error| error.to_string());
        emit(&format!(
            "destination {}: {} branch {branch}",
            self.target, destination.url
        ))?;
        for module in &build.modules {
            emit(&format!(
                "module {}: {}, {} bytes ({})",
                module.name,
                module.kind.as_str(),
                module.len,
                module.path.display()
            ))?;
        }
        for skipped in &build.skipped {
            emit(&format!(
                "skipped {}: {}",
                skipped.path.display(),
                skipped.reason
            ))?;
        }
        if self.dry_run {
            return emit("dry run: nothing uploaded");
        }
        let mut client = Client::sign_in(destination.url, &destination.credentials)?;
        if !client.branches()?.contains(&branch) {
            client.create_branch(&branch, &build.modules)?;
            emit(&format!("created branch {branch}"))?;
        }
        client.set_code(&branch, &build.modules)?;
        emit(&format!(
            "uploaded {} modules to branch {branch}",
            build.modules.len()
        ))?;
        for active in self.activate {
            client.set_active_branch(&branch, active)?;
            emit(&format!("activated branch {branch} in {}", active.as_str()))?;
        }
        Ok(())
    }
}

/// The current branch of the git repository of the working directory.
fn git_branch() -> Result<String, String> {
    let output = Process::new("git")
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(match stderr.trim() {
            "" => "HEAD is detached: no current git branch".to_owned(),
            error => format!("git: {error}"),
        });
    }
    let mut branch = String::from_utf8(output.stdout)
        .map_err(|_| "git: the branch name is not UTF-8".to_owned())?;
    branch.truncate(branch.trim_end().len());
    Ok(branch)
}
