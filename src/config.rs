//! The configuration file `screepsmanager.toml`: the build directory, the
//! servers, and the profiles that upload to them.
//!
//! ```toml
//! dir = "dist"         # the build directory, relative to this file (default: dist)
//! default = ["main"]   # the profiles of `upload` without --profile
//!
//! [servers.official]
//! url = "https://screeps.com"
//! token = { env = "SCREEPS_TOKEN" }
//!
//! [servers.season]
//! url = "https://screeps.com/season"
//! token = { env = "SCREEPS_TOKEN" }
//!
//! [servers.local]
//! url = "http://127.0.0.1:21025"
//! email = "me@example.com"
//! password = "secret"
//!
//! [profiles.main]
//! server = "official"
//! branch = "main"
//! activate = ["world"]
//!
//! [profiles.sim]
//! server = "official"
//! branch = "sim"
//! activate = ["sim"]
//!
//! [profiles.dev]
//! server = "local"
//! branch = "auto"
//! ```
//!
//! A server is the URL that the API is under (`http` or `https`) and the
//! credentials of an account on it: `token`, or `email` and `password` for
//! a private server with password sign-in (screepsmod-auth). A secret is a
//! string, or `{ env = "VARIABLE" }` to read it from the environment, or
//! else from the `--env-file` ([`EnvFile`](crate::envfile::EnvFile)), when
//! a command needs it. A profile is a server, a branch (`default` when not
//! set; `auto` is the current git branch), and where the upload makes the
//! branch run (`world`, `sim`). A key that the format does not have is an
//! error.
//!
//! A profile with a spawn table is one that `poll` keeps a spawn of:
//!
//! ```toml
//! [profiles.main.spawn]
//! selector = ["node", "scripts/select-spawn.mjs"]  # argv, no shell; runs next to this file
//! shard = "auto"      # the one shard with CPU, a shard name, or unset without shards
//! interval = 60       # seconds between polls (60..=86400)
//! radius = 5          # rooms around the start room that candidates come from (1..=10)
//! candidates = 16     # the most rooms that the selector chooses from (1..=64)
//! ```
//!
//! [`name`] checks the names, [`secret`] holds and resolves the secrets,
//! [`server`] the URLs and the sign-in of the servers, and [`spawn`] the
//! spawn tables; this module ties profiles to servers and selects them.

mod name;
mod secret;
mod server;
mod spawn;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub(crate) use name::{ProfileName, ServerName};
pub(crate) use secret::{SecretError, Sensitive};
pub(crate) use server::{Credentials, Server, ServerUrl};
pub(crate) use spawn::{ShardChoice, Spawn};

use crate::branch::{Active, Branch};

/// The name of the configuration file that `upload` and `profiles` look
/// for in the working directory and its parents.
pub(crate) const FILE_NAME: &str = "screepsmanager.toml";
/// The build directory when the configuration names none.
const DEFAULT_DIR: &str = "dist";

/// A profile: a server, a branch, where the upload makes it run, and how
/// `poll` keeps a spawn in the world.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    /// The server.
    pub(crate) server: ServerName,
    /// The branch.
    #[serde(default)]
    pub(crate) branch: Branch,
    /// Where the upload makes the branch run.
    #[serde(default)]
    pub(crate) activate: Vec<Active>,
    /// How `poll` places the spawn of the account; `poll` skips a profile
    /// without it.
    pub(crate) spawn: Option<Spawn>,
}

/// The configuration as the file writes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    dir: Option<PathBuf>,
    #[serde(default)]
    default: Vec<ProfileName>,
    #[serde(default)]
    servers: BTreeMap<ServerName, Server>,
    #[serde(default)]
    profiles: BTreeMap<ProfileName, Profile>,
}

/// A configuration whose profiles name its servers.
#[derive(Debug)]
pub(crate) struct Config {
    /// The file.
    pub(crate) path: PathBuf,
    /// The build directory, relative to the file.
    pub(crate) dir: PathBuf,
    servers: BTreeMap<ServerName, Server>,
    profiles: BTreeMap<ProfileName, Profile>,
    default: Vec<ProfileName>,
}

/// A profile of a configuration with its name and its server.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Selected<'a> {
    /// The name of the profile.
    pub(crate) name: &'a ProfileName,
    /// The profile.
    pub(crate) profile: &'a Profile,
    /// The server of the profile.
    pub(crate) server: &'a Server,
}

impl Config {
    /// The configuration of the file `path`, or without one of the nearest
    /// [`FILE_NAME`] in the working directory and its parents.
    ///
    /// # Errors
    ///
    /// When no file is found, the file is not readable, or it is not valid.
    pub(crate) fn load(path: Option<&Path>) -> Result<Self, Error> {
        let path = if let Some(path) = path {
            path.to_owned()
        } else {
            let dir = std::env::current_dir().map_err(Error::WorkingDir)?;
            let found = dir
                .ancestors()
                .map(|dir| dir.join(FILE_NAME))
                .find(|path| path.is_file());
            found.ok_or(Error::NotFound { dir })?
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text, path),
            Err(source) => Err(Error::Read { path, source }),
        }
    }

    /// The configuration of the text `text` of the file `path`.
    ///
    /// # Errors
    ///
    /// When the text is not a valid configuration.
    pub(crate) fn parse(text: &str, path: PathBuf) -> Result<Self, Error> {
        let file: File = match toml::from_str(text) {
            Ok(file) => file,
            Err(source) => return Err(Error::Parse { path, source }),
        };
        for (name, profile) in &file.profiles {
            if !file.servers.contains_key(&profile.server) {
                return Err(Error::UnknownServer {
                    path,
                    profile: name.clone(),
                    server: profile.server.clone(),
                    servers: file.servers.keys().cloned().collect(),
                });
            }
        }
        for name in &file.default {
            if !file.profiles.contains_key(name) {
                return Err(Error::UnknownDefault {
                    path,
                    profile: name.clone(),
                    profiles: file.profiles.keys().cloned().collect(),
                });
            }
        }
        let dir = path
            .parent()
            .unwrap_or(Path::new(""))
            .join(file.dir.as_deref().unwrap_or(Path::new(DEFAULT_DIR)));
        Ok(Self {
            path,
            dir,
            servers: file.servers,
            profiles: file.profiles,
            default: file.default,
        })
    }

    /// The profiles `names` in order, without repeats. Without names, the
    /// `default` profiles, or else the only profile.
    ///
    /// # Errors
    ///
    /// When a name is not a profile, or no name is given and the
    /// configuration has no default and not exactly one profile.
    pub(crate) fn select(&self, names: &[ProfileName]) -> Result<Vec<Selected<'_>>, Error> {
        let names = match (names, self.default.as_slice()) {
            ([], []) => {
                let mut all = self.profiles();
                return match (all.next(), all.next()) {
                    (Some(only), None) => Ok(vec![only]),
                    (None, _) => Err(Error::NoProfile {
                        path: self.path.clone(),
                    }),
                    (Some(_), Some(_)) => Err(Error::Ambiguous {
                        path: self.path.clone(),
                        profiles: self.profiles.keys().cloned().collect(),
                    }),
                };
            }
            ([], default) => default,
            (names, _) => names,
        };
        let mut selected: Vec<Selected<'_>> = Vec::with_capacity(names.len());
        for name in names {
            if selected.iter().all(|other| other.name != name) {
                selected.push(self.profile(name)?);
            }
        }
        Ok(selected)
    }

    /// Every profile, in name order.
    pub(crate) fn profiles(&self) -> impl Iterator<Item = Selected<'_>> {
        self.profiles.iter().filter_map(|(name, profile)| {
            let server = self.servers.get(&profile.server)?;
            Some(Selected {
                name,
                profile,
                server,
            })
        })
    }

    /// Whether `upload` without `--profile` uploads to the profile `name`.
    pub(crate) fn is_default(&self, name: &ProfileName) -> bool {
        if self.default.is_empty() {
            self.profiles.len() == 1 && self.profiles.contains_key(name)
        } else {
            self.default.contains(name)
        }
    }

    /// The profiles of `poll`: `names` in order, without repeats, each with
    /// a spawn table; without names, every profile with one. Two profiles
    /// of one server are one account, which one poll alone may reset.
    ///
    /// # Errors
    ///
    /// When a name is not a profile or has no spawn table, no profile has
    /// one, or two profiles share a server.
    pub(crate) fn select_spawn(&self, names: &[ProfileName]) -> Result<Vec<Selected<'_>>, Error> {
        let selected: Vec<Selected<'_>> = if names.is_empty() {
            self.profiles()
                .filter(|target| target.profile.spawn.is_some())
                .collect()
        } else {
            let mut selected: Vec<Selected<'_>> = Vec::with_capacity(names.len());
            for name in names {
                if selected.iter().all(|other| other.name != name) {
                    let target = self.profile(name)?;
                    if target.profile.spawn.is_none() {
                        return Err(Error::NoSpawn {
                            path: self.path.clone(),
                            profile: name.clone(),
                        });
                    }
                    selected.push(target);
                }
            }
            selected
        };
        if selected.is_empty() {
            return Err(Error::NoSpawnProfile {
                path: self.path.clone(),
            });
        }
        for (at, first) in selected.iter().enumerate() {
            if let Some(second) = selected[at + 1..]
                .iter()
                .find(|other| other.profile.server == first.profile.server)
            {
                return Err(Error::SharedServer {
                    path: self.path.clone(),
                    first: first.name.clone(),
                    second: second.name.clone(),
                    server: first.profile.server.clone(),
                });
            }
        }
        Ok(selected)
    }

    /// The directory of the file, where the spawn selector runs.
    pub(crate) fn root(&self) -> &Path {
        match self.path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        }
    }

    /// The profile `name`.
    fn profile(&self, name: &ProfileName) -> Result<Selected<'_>, Error> {
        let unknown = || Error::UnknownProfile {
            path: self.path.clone(),
            profile: name.clone(),
            profiles: self.profiles.keys().cloned().collect(),
        };
        let (name, profile) = self.profiles.get_key_value(name).ok_or_else(unknown)?;
        let server = self.servers.get(&profile.server).ok_or_else(unknown)?;
        Ok(Selected {
            name,
            profile,
            server,
        })
    }
}

/// `names`, separated by commas.
fn list<T: fmt::Display>(names: &[T]) -> String {
    let names: Vec<String> = names.iter().map(ToString::to_string).collect();
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

/// A configuration that is not available or not valid.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The working directory is not available.
    #[error("the working directory: {0}")]
    WorkingDir(#[source] std::io::Error),
    /// No configuration file in the working directory or its parents.
    #[error(
        "no {FILE_NAME} in {} or its parents; create one or pass --config FILE",
        .dir.display()
    )]
    NotFound {
        /// The working directory.
        dir: PathBuf,
    },
    /// The file is not readable.
    #[error("{}: {source}", .path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// The read error.
        source: std::io::Error,
    },
    /// The file is not a valid configuration.
    #[error("{}: {source}", .path.display())]
    Parse {
        /// The file.
        path: PathBuf,
        /// Where and why.
        source: toml::de::Error,
    },
    /// A profile names a server that the file does not have.
    #[error(
        "{}: profile {profile}: no server {server}; servers: {}",
        .path.display(),
        list(.servers)
    )]
    UnknownServer {
        /// The file.
        path: PathBuf,
        /// The profile.
        profile: ProfileName,
        /// The server that it names.
        server: ServerName,
        /// The servers of the file.
        servers: Vec<ServerName>,
    },
    /// `default` names a profile that the file does not have.
    #[error(
        "{}: default: no profile {profile}; profiles: {}",
        .path.display(),
        list(.profiles)
    )]
    UnknownDefault {
        /// The file.
        path: PathBuf,
        /// The profile that `default` names.
        profile: ProfileName,
        /// The profiles of the file.
        profiles: Vec<ProfileName>,
    },
    /// A selected profile that the file does not have.
    #[error("{}: no profile {profile}; profiles: {}", .path.display(), list(.profiles))]
    UnknownProfile {
        /// The file.
        path: PathBuf,
        /// The profile.
        profile: ProfileName,
        /// The profiles of the file.
        profiles: Vec<ProfileName>,
    },
    /// A profile of `poll` has no spawn table.
    #[error("{}: profile {profile}: no [profiles.{profile}.spawn] table", .path.display())]
    NoSpawn {
        /// The file.
        path: PathBuf,
        /// The profile.
        profile: ProfileName,
    },
    /// `poll` without `--profile`, and no profile has a spawn table.
    #[error("{}: no profile has a [profiles.NAME.spawn] table", .path.display())]
    NoSpawnProfile {
        /// The file.
        path: PathBuf,
    },
    /// Two profiles of `poll` share a server, and so an account.
    #[error(
        "{}: profiles {first} and {second} share server {server}: poll one profile per account",
        .path.display()
    )]
    SharedServer {
        /// The file.
        path: PathBuf,
        /// The first profile.
        first: ProfileName,
        /// The second profile.
        second: ProfileName,
        /// The server.
        server: ServerName,
    },
    /// The file has no profile.
    #[error("{}: no profile; add a [profiles.NAME] table", .path.display())]
    NoProfile {
        /// The file.
        path: PathBuf,
    },
    /// No profile is selected, and the file has several and no default.
    #[error(
        "{}: profiles {}: choose with --profile, or set `default`",
        .path.display(),
        list(.profiles)
    )]
    Ambiguous {
        /// The file.
        path: PathBuf,
        /// The profiles of the file.
        profiles: Vec<ProfileName>,
    },
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::secret::Secret;
    use super::server::Auth;
    use super::*;

    /// A server and one profile without a spawn table.
    const SERVER_ONLY_PROFILE: &str =
        "[servers.s]\nurl = \"https://screeps.com\"\ntoken = \"t\"\n[profiles.p]\nserver = \"s\"\n";

    /// The configuration of the module documentation, with `default`.
    const EXAMPLE: &str = r#"
        default = ["main"]

        [servers.official]
        url = "https://screeps.com"
        token = { env = "SCREEPS_TOKEN" }

        [servers.season]
        url = "https://screeps.com/season"
        token = "t"

        [servers.local]
        url = "http://127.0.0.1:21025"
        email = "me@example.com"
        password = "p"

        [profiles.main]
        server = "official"
        branch = "main"
        activate = ["world"]

        [profiles.sim]
        server = "official"
        branch = "sim"
        activate = ["sim"]

        [profiles.season]
        server = "season"

        [profiles.dev]
        server = "local"
        branch = "auto"
    "#;

    /// The configuration of `text` in `project/screepsmanager.toml`, which
    /// must be valid.
    fn parsed(text: &str) -> Config {
        Config::parse(text, PathBuf::from("project").join(FILE_NAME))
            .expect("a valid configuration")
    }

    /// The names of `selected`.
    fn names(selected: &[Selected<'_>]) -> Vec<String> {
        selected.iter().map(|s| s.name.to_string()).collect()
    }

    /// Profile names.
    fn profiles(names: &[&str]) -> Vec<ProfileName> {
        names
            .iter()
            .map(|name| name.parse().expect("a valid name"))
            .collect()
    }

    /// Servers, secrets, branches, and the build directory resolve; server
    /// URLs end in `/` so that API paths join under them.
    #[test]
    fn example() {
        let config = parsed(EXAMPLE);
        assert_eq!(config.dir, Path::new("project").join(DEFAULT_DIR));
        let [main, sim, season, dev] = ["main", "sim", "season", "dev"].map(|name| {
            *config
                .select(&profiles(&[name]))
                .expect("a profile")
                .first()
                .expect("one")
        });
        assert_eq!(
            main.server.auth,
            Auth::Token(Secret::Env("SCREEPS_TOKEN".to_owned()))
        );
        assert_eq!(main.profile.activate, [Active::World]);
        assert_eq!(sim.profile.branch.to_string(), "sim");
        assert_eq!(
            season.server.url.join("api/user/code").map(String::from),
            Ok("https://screeps.com/season/api/user/code".to_owned())
        );
        assert_eq!(season.profile.branch, Branch::default());
        assert_eq!(dev.server.url.to_string(), "http://127.0.0.1:21025/");
        assert_eq!(dev.profile.branch, Branch::Auto);
        assert!(matches!(
            &dev.server.auth,
            Auth::Password { email, password: Secret::Value(_) } if email == "me@example.com"
        ));
    }

    /// `--profile` names win, in order and without repeats; then `default`;
    /// then the only profile.
    #[test]
    fn selection() {
        let config = parsed(EXAMPLE);
        let selected = config.select(&profiles(&["sim", "main", "sim"]));
        assert_eq!(
            selected.map(|s| names(&s)).ok(),
            Some(vec!["sim".to_owned(), "main".to_owned()])
        );
        assert_eq!(
            config.select(&[]).map(|s| names(&s)).ok(),
            Some(vec!["main".to_owned()])
        );
        let error = config
            .select(&profiles(&["prod"]))
            .expect_err("no profile prod");
        assert!(
            error
                .to_string()
                .contains("profiles: dev, main, season, sim"),
            "{error}"
        );

        let without_default = parsed(&EXAMPLE.replace(r#"default = ["main"]"#, ""));
        assert!(matches!(
            without_default.select(&[]),
            Err(Error::Ambiguous { .. })
        ));
        let only = parsed(
            "[servers.s]\nurl = \"https://screeps.com\"\ntoken = \"t\"\n[profiles.p]\nserver = \"s\"\n",
        );
        assert_eq!(
            only.select(&[]).map(|s| names(&s)).ok(),
            Some(vec!["p".to_owned()])
        );
        assert!(only.is_default(&"p".parse().expect("a name")));
    }

    /// Invalid configurations name the problem.
    #[test]
    fn invalid() {
        let server = "[servers.s]\nurl = \"https://screeps.com\"\n";
        for (text, expected) in [
            (
                format!("{server}token = \"t\"\nhostname = \"x\"\n"),
                "hostname",
            ),
            (format!("{server}email = \"e\"\n"), "set `token`"),
            (
                format!("{server}token = \"t\"\nemail = \"e\"\npassword = \"p\"\n"),
                "not both",
            ),
            (format!("{server}token = \"\"\n"), "not empty"),
            (format!("{server}token = {{ file = \"x\" }}\n"), "file"),
            (
                "[servers.s]\nurl = \"ftp://screeps.com\"\ntoken = \"t\"\n".to_owned(),
                "use http or https",
            ),
            (
                format!("{server}token = \"t\"\n[profiles.p]\nserver = \"x\"\n"),
                "no server x; servers: s",
            ),
            (
                format!(
                    "{server}token = \"t\"\n[profiles.p]\nserver = \"s\"\nactivate = [\"moon\"]\n"
                ),
                "moon",
            ),
            (
                format!("{server}token = \"t\"\n[profiles.p]\nserver = \"s\"\nbranch = \"\"\n"),
                "branch",
            ),
            (
                format!("default = [\"q\"]\n{server}token = \"t\"\n[profiles.p]\nserver = \"s\"\n"),
                "default: no profile q",
            ),
            (
                "[servers.\"a b\"]\nurl = \"https://screeps.com\"\ntoken = \"t\"\n".to_owned(),
                "a name has",
            ),
        ] {
            let error = Config::parse(&text, PathBuf::from(FILE_NAME)).expect_err(&text);
            assert!(error.to_string().contains(expected), "{text}\n{error}");
        }
    }

    /// A spawn table: the selector, `auto` or a shard name or no shard, and
    /// bounded numbers with defaults.
    #[test]
    fn spawn() {
        let text = format!(
            "{}[profiles.main.spawn]\nselector = [\"node\", \"scripts/select-spawn.mjs\"]\nshard = \"auto\"\n\
             [profiles.dev.spawn]\nselector = [\"sel\"]\ninterval = 120\nradius = 3\ncandidates = 4\n\
             [profiles.season.spawn]\nselector = [\"sel\"]\nshard = \"shardSeason\"\n",
            EXAMPLE.replace(r#"default = ["main"]"#, "")
        );
        let config = parsed(&text);
        let spawn = |name: &str| {
            config.select(&profiles(&[name])).expect("a profile")[0]
                .profile
                .spawn
                .clone()
        };
        let main = spawn("main").expect("a spawn table");
        assert_eq!(main.selector, ["node", "scripts/select-spawn.mjs"]);
        assert_eq!(main.shard, Some(ShardChoice::Auto));
        assert_eq!(main.interval, Duration::from_secs(60));
        assert_eq!((main.radius, main.candidates), (5, 16));
        let dev = spawn("dev").expect("a spawn table");
        assert_eq!(dev.shard, None);
        assert_eq!(dev.interval, Duration::from_secs(120));
        assert_eq!((dev.radius, dev.candidates), (3, 4));
        assert_eq!(
            spawn("season").and_then(|s| s.shard).map(|s| s.to_string()),
            Some("shardSeason".to_owned())
        );
        assert_eq!(spawn("sim"), None);
        assert_eq!(config.root(), Path::new("project"));
        let bare = Config::parse(SERVER_ONLY_PROFILE, PathBuf::from(FILE_NAME))
            .expect("a valid configuration");
        assert_eq!(bare.root(), Path::new("."));

        let all = config.select_spawn(&[]).expect("spawn profiles");
        assert_eq!(names(&all), ["dev", "main", "season"]);
        assert!(matches!(
            config.select_spawn(&profiles(&["sim"])),
            Err(Error::NoSpawn { .. })
        ));
        assert!(matches!(
            parsed(EXAMPLE).select_spawn(&[]),
            Err(Error::NoSpawnProfile { .. })
        ));
        let shared = text.replace(
            "[profiles.dev]\n        server = \"local\"",
            "[profiles.dev]\n        server = \"official\"",
        );
        assert_ne!(shared, text, "the replacement applies");
        assert!(matches!(
            parsed(&shared).select_spawn(&profiles(&["main", "dev"])),
            Err(Error::SharedServer { .. })
        ));
    }

    /// Spawn tables that are not valid name the problem.
    #[test]
    fn invalid_spawn() {
        let base = format!("{SERVER_ONLY_PROFILE}[profiles.p.spawn]\n");
        for (text, expected) in [
            (format!("{base}selector = []\n"), "`selector`"),
            (format!("{base}selector = [\"\"]\n"), "`selector`"),
            (format!("{base}shard = \"auto\"\n"), "selector"),
            (
                format!("{base}selector = [\"s\"]\ninterval = 5\n"),
                "`interval` = 5: use 60..=86400",
            ),
            (
                format!("{base}selector = [\"s\"]\nradius = 0\n"),
                "`radius` = 0",
            ),
            (
                format!("{base}selector = [\"s\"]\ncandidates = 65\n"),
                "`candidates` = 65",
            ),
            (
                format!("{base}selector = [\"s\"]\nshard = \"a b\"\n"),
                "a name has",
            ),
            (format!("{base}selector = [\"s\"]\nname = \"x\"\n"), "name"),
        ] {
            let error = Config::parse(&text, PathBuf::from(FILE_NAME)).expect_err(&text);
            assert!(error.to_string().contains(expected), "{text}\n{error}");
        }
    }
}
