//! The `upload` command: reads the modules of the build directory once and
//! uploads them to the branch of each selected profile, in order. Profiles
//! of one server share one sign-in; the first failure stops the upload.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::api::{Access, Client};
use crate::branch::{Active, Branch, BranchName};
use crate::config::{Config, ProfileName, Selected, ServerName};
use crate::envfile::EnvFile;
use crate::modules::{self, Modules};

/// The arguments of `upload`.
#[derive(Debug, clap::Args)]
pub(crate) struct Upload {
    /// The build directory [default: the `dir` of the configuration, or
    /// dist next to it]
    dir: Option<PathBuf>,
    /// A profile to upload to (repeatable, or comma-separated) [default: the
    /// `default` profiles of the configuration, or its only profile]
    #[arg(
        long = "profile",
        short,
        value_name = "NAME",
        env = "SCREEPSMANAGER_PROFILE",
        value_delimiter = ','
    )]
    profiles: Vec<ProfileName>,
    /// The branch of every profile instead of its own; auto is the current
    /// git branch
    #[arg(long, short, value_name = "BRANCH")]
    branch: Option<Branch>,
    /// Also make the branch run in the world or in the simulator
    /// (repeatable, or comma-separated)
    #[arg(long, value_enum, value_name = "WHERE", value_delimiter = ',')]
    activate: Vec<Active>,
    /// Read the configuration, the secrets, and the modules, print the
    /// plan, and upload nothing
    #[arg(long)]
    dry_run: bool,
}

impl Upload {
    /// Runs the upload with `config` and the `--env-file` variables `env`,
    /// and prints each step to `out`.
    pub(crate) fn run(
        self,
        config: &Config,
        env: &EnvFile,
        out: &mut dyn Write,
    ) -> Result<(), Error> {
        let targets = config.select(&self.profiles)?;
        let dir = self.dir.as_deref().unwrap_or(&config.dir);
        let build = modules::read(dir)?;
        writeln!(
            out,
            "build {}: {} modules",
            dir.display(),
            build.modules.len()
        )?;
        let width = build
            .modules
            .iter()
            .map(|(name, _)| name.as_str().len())
            .max()
            .unwrap_or_default();
        for (name, module) in build.modules.iter() {
            writeln!(
                out,
                "  {name:<width$}  {:<10}  {:>9} bytes  {}",
                module.code.kind(),
                module.len,
                file_name(&module.file).display()
            )?;
        }
        for skipped in &build.skipped {
            writeln!(
                out,
                "  skipped {}: {}",
                file_name(&skipped.path).display(),
                skipped.reason
            )?;
        }
        let mut git_branch = None;
        let mut clients: BTreeMap<&ServerName, Client> = BTreeMap::new();
        for target in targets {
            let branch = self
                .branch
                .as_ref()
                .unwrap_or(&target.profile.branch)
                .resolve(&mut git_branch)?;
            let activate: Vec<Active> = Active::ALL
                .into_iter()
                .filter(|active| {
                    self.activate.contains(active) || target.profile.activate.contains(active)
                })
                .collect();
            write!(
                out,
                "profile {}: branch {branch} on {}",
                target.name, target.server.url
            )?;
            for (at, active) in activate.iter().enumerate() {
                write!(
                    out,
                    "{} {active}",
                    if at == 0 { ", run in" } else { " and" }
                )?;
            }
            writeln!(out)?;
            let credentials = target
                .server
                .auth
                .resolve(env)
                .map_err(|source| Error::Secret {
                    server: target.profile.server.clone(),
                    source,
                })?;
            if self.dry_run {
                writeln!(out, "  dry run: nothing uploaded")?;
                continue;
            }
            let client = match clients.entry(&target.profile.server) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => entry.insert(
                    Client::sign_in(&target.server.url, &credentials, Access::ReadWrite)
                        .map_err(|source| api_error(&target, source))?,
                ),
            };
            upload(client, &target, &branch, &build.modules, &activate, out)?;
        }
        Ok(())
    }
}

/// Uploads `modules` to the branch `branch` of the profile `target` with
/// `client`, and makes the branch run in each of `activate`.
fn upload(
    client: &mut Client,
    target: &Selected<'_>,
    branch: &BranchName,
    modules: &Modules,
    activate: &[Active],
    out: &mut dyn Write,
) -> Result<(), Error> {
    let api = |source| api_error(target, source);
    let branches = client.branches().map_err(api)?;
    let info = branches.iter().find(|info| info.branch == branch.as_str());
    if info.is_none() {
        client.create_branch(branch, modules).map_err(api)?;
        writeln!(out, "  created branch {branch}")?;
    }
    client.set_code(branch, modules).map_err(api)?;
    writeln!(out, "  uploaded {} modules", modules.len())?;
    for &active in activate {
        if info.is_some_and(|info| info.runs_in(active)) {
            writeln!(out, "  branch {branch} already runs in {active}")?;
        } else {
            client.set_active_branch(branch, active).map_err(api)?;
            writeln!(out, "  branch {branch} runs in {active}")?;
        }
    }
    Ok(())
}

/// The error of the call `source` for the profile `target`.
fn api_error(target: &Selected<'_>, source: crate::api::Error) -> Error {
    Error::Api {
        profile: target.name.clone(),
        source,
    }
}

/// The last component of `path`, which is in the build directory.
fn file_name(path: &Path) -> &Path {
    path.file_name().map_or(path, Path::new)
}
