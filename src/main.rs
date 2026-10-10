//! `screepsmanager`: uploads built Screeps code to the branches of Screeps
//! servers, and keeps a spawn of the account in the world. Run
//! `screepsmanager help` for usage.
//!
//! The configuration ([`config`]) names servers and profiles: a profile is
//! a server, a branch, where the branch runs, and how to place its spawn.
//! `upload` reads the modules of the build directory ([`modules`]) and
//! replaces the code of the branch of each selected profile through the
//! Screeps API ([`api`]). `poll` ([`poll`]) places the spawn of an empty
//! account where the selector of the bot ([`selector`]) chooses, and resets
//! an account that its heartbeat ([`heartbeat`]) proves wiped out.
//! `--env-file` ([`envfile`]) supplies the variables of the secrets.

mod api;
mod branch;
mod config;
mod envfile;
mod heartbeat;
mod modules;
mod poll;
mod room;
mod selector;
mod upload;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::config::{Config, ProfileName, ServerName};
use crate::envfile::EnvFile;

/// Uploads built Screeps code to the branches of Screeps servers, and keeps
/// a spawn of the account in the world.
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
    /// A dotenv file of the variables of `{ env = "VARIABLE" }` secrets; a
    /// variable of the environment wins over the file [default: none]
    #[arg(
        long,
        global = true,
        env = "SCREEPSMANAGER_ENV_FILE",
        value_name = "FILE"
    )]
    env_file: Option<PathBuf>,
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
    /// Keep a spawn of the account of each profile with a spawn table in
    /// the world.
    ///
    /// Each poll reads the account and its world status. An empty account
    /// gets its spawn where the selector of the profile chooses among
    /// candidate rooms, after poll checks the room and the tile again. A
    /// lost account (objects, no spawn) is reset only when the heartbeat of
    /// the bot on every lost shard is fresh and counts no foothold, for 180
    /// s in a row and again right before. Without --execute, poll changes
    /// nothing and prints what it would do.
    Poll(poll::Poll),
}

/// A command that failed.
#[derive(Debug, thiserror::Error)]
enum Error {
    /// The configuration is not available or not valid.
    #[error(transparent)]
    Config(#[from] config::Error),
    /// The env file is not available or not valid.
    #[error(transparent)]
    EnvFile(#[from] envfile::Error),
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
    /// A poll of a profile failed.
    #[error("profile {profile}: {source}")]
    Poll {
        /// The profile.
        profile: ProfileName,
        /// What failed.
        source: poll::Failure,
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
    let env = EnvFile::load(cli.env_file.as_deref())?;
    if let Some(path) = &env.path {
        writeln!(out, "env file {}: {} variables", path.display(), env.len())?;
    }
    match cli.command {
        Command::Upload(upload) => upload.run(&config, &env, out),
        Command::Profiles => profiles(&config, out),
        Command::Poll(poll) => poll.run(&config, &env, out),
    }
}

/// Prints the profiles of `config` as a table. A `*` marks the profiles of
/// `upload` without `--profile`; without one, a last line says that `upload`
/// needs `--profile`.
fn profiles(config: &Config, out: &mut dyn Write) -> Result<(), Error> {
    let mut rows =
        vec![["PROFILE", "SERVER", "URL", "BRANCH", "ACTIVATE", "AUTH"].map(String::from)];
    let mut marked = false;
    for target in config.profiles() {
        let default = config.is_default(target.name);
        marked |= default;
        let mark = if default { " *" } else { "" };
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
    if marked {
        writeln!(out, "* the profiles of `upload` without --profile")?;
    } else if rows.len() > 1 {
        writeln!(
            out,
            "no default profile: `upload` needs --profile, or set `default`"
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server for the configurations of the tests.
    const SERVER: &str = "[servers.s]\nurl = \"https://screeps.com\"\ntoken = \"t\"\n";

    /// The `profiles` output of the configuration `text`.
    fn listing(text: &str) -> String {
        let config =
            Config::parse(text, PathBuf::from(config::FILE_NAME)).expect("a valid configuration");
        let mut out = Vec::new();
        profiles(&config, &mut out).expect("the listing writes");
        String::from_utf8(out).expect("UTF-8 output")
    }

    /// The legend of the `*` mark comes only with a marked profile; several
    /// profiles without `default` say that `upload` needs `--profile`
    /// instead (issue #1).
    #[test]
    fn default_legend() {
        let two = format!("{SERVER}[profiles.a]\nserver = \"s\"\n[profiles.b]\nserver = \"s\"\n");
        let unmarked = listing(&two);
        assert!(!unmarked.contains('*'), "{unmarked}");
        assert!(unmarked.contains("`upload` needs --profile"), "{unmarked}");

        let with_default = listing(&format!("default = [\"b\"]\n{two}"));
        assert!(with_default.contains("b *"), "{with_default}");
        assert!(with_default.ends_with("* the profiles of `upload` without --profile\n"));

        let only = listing(&format!("{SERVER}[profiles.a]\nserver = \"s\"\n"));
        assert!(only.contains("a *"), "{only}");
        assert!(only.ends_with("* the profiles of `upload` without --profile\n"));
    }
}
