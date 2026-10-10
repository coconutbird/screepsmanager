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
//! else from the `--env-file` ([`EnvFile`]), when a command needs it. A
//! profile is a server, a branch (`default` when not set; `auto` is the
//! current git branch), and where the upload makes the branch run
//! (`world`, `sim`). A key that the format does not have is an error.
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

use std::collections::BTreeMap;
use std::fmt;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use url::Url;

use crate::branch::{Active, Branch};
use crate::envfile::EnvFile;

/// The name of the configuration file that `upload` and `profiles` look
/// for in the working directory and its parents.
pub(crate) const FILE_NAME: &str = "screepsmanager.toml";
/// The build directory when the configuration names none.
const DEFAULT_DIR: &str = "dist";

/// Defines a name type: letters, digits, `-`, `_`, and `.`.
macro_rules! name {
    ($(#[$doc:meta])* $name:ident, $what:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
        #[serde(try_from = "String")]
        pub(crate) struct $name(String);

        impl TryFrom<String> for $name {
            type Error = InvalidName;

            fn try_from(name: String) -> Result<Self, InvalidName> {
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
                {
                    Ok(Self(name))
                } else {
                    Err(InvalidName { what: $what, name })
                }
            }
        }

        impl FromStr for $name {
            type Err = InvalidName;

            fn from_str(name: &str) -> Result<Self, InvalidName> {
                Self::try_from(name.to_owned())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.pad(&self.0)
            }
        }
    };
}

name!(
    /// The name of a server of the configuration.
    ServerName,
    "server"
);
name!(
    /// The name of a profile of the configuration.
    ProfileName,
    "profile"
);
name!(
    /// The name of a shard of a server (`shard3`).
    ShardName,
    "shard"
);

impl ShardName {
    /// The name as the API takes it.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// A server or profile name with other characters than letters, digits,
/// `-`, `_`, and `.`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{what} {name:?}: a name has letters, digits, `-`, `_`, and `.`")]
pub(crate) struct InvalidName {
    what: &'static str,
    name: String,
}

/// The URL of a server: `http` or `https`, without a query or a fragment,
/// with a final `/` so that the API paths join under it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct ServerUrl(Url);

impl ServerUrl {
    /// The URL of the API path `path` (`api/user/code`).
    ///
    /// # Errors
    ///
    /// When the joined URL does not parse.
    pub(crate) fn join(&self, path: &str) -> Result<Url, url::ParseError> {
        self.0.join(path)
    }
}

impl TryFrom<String> for ServerUrl {
    type Error = InvalidUrl;

    fn try_from(text: String) -> Result<Self, InvalidUrl> {
        let mut url = match Url::parse(&text) {
            Ok(url) => url,
            Err(source) => return Err(InvalidUrl::Parse { url: text, source }),
        };
        if !matches!(url.scheme(), "http" | "https") {
            return Err(InvalidUrl::Scheme { url: text });
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(InvalidUrl::Query { url: text });
        }
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }
        Ok(Self(url))
    }
}

impl fmt::Display for ServerUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A server URL that is not valid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum InvalidUrl {
    /// It does not parse.
    #[error("url {url:?}: {source}")]
    Parse {
        /// The URL.
        url: String,
        /// Why it does not parse.
        source: url::ParseError,
    },
    /// Its scheme is not `http` or `https`.
    #[error("url {url:?}: use http or https")]
    Scheme {
        /// The URL.
        url: String,
    },
    /// It has a query or a fragment.
    #[error("url {url:?}: a server URL has no query and no fragment")]
    Query {
        /// The URL.
        url: String,
    },
}

/// A secret value. Its debug output does not show it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Sensitive(String);

impl Sensitive {
    /// The secret `value`.
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    /// The value.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Sensitive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sensitive(..)")
    }
}

/// A secret of a server: written in the file, or read from an environment
/// variable when an upload needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Secret {
    /// The value, from the file.
    Value(Sensitive),
    /// The environment variable of the value.
    Env(String),
}

impl Secret {
    /// The value of the secret: a variable comes from the process
    /// environment, or else from the `--env-file` variables `env`.
    ///
    /// # Errors
    ///
    /// When the environment variable is not set, not Unicode, or empty.
    pub(crate) fn resolve(&self, env: &EnvFile) -> Result<Sensitive, SecretError> {
        self.resolve_with(env, &|name: &str| std::env::var(name))
    }

    /// The value of the secret, with `process` for the process environment.
    fn resolve_with(
        &self,
        env: &EnvFile,
        process: &dyn Fn(&str) -> Result<String, std::env::VarError>,
    ) -> Result<Sensitive, SecretError> {
        match self {
            Self::Value(value) => Ok(value.clone()),
            Self::Env(variable) => match env.var(variable, process) {
                Ok(Some(value)) if value.expose().is_empty() => {
                    Err(SecretError::Empty(variable.clone()))
                }
                Ok(Some(value)) => Ok(value),
                Ok(None) => Err(SecretError::NotSet(variable.clone())),
                Err(_) => Err(SecretError::NotUnicode(variable.clone())),
            },
        }
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(_) => f.write_str("in the file"),
            Self::Env(variable) => write!(f, "from ${variable}"),
        }
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The table form of a secret.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FromEnv {
            env: String,
        }

        /// Reads a string or a `{ env = "VARIABLE" }` table.
        struct SecretVisitor;

        impl<'de> Visitor<'de> for SecretVisitor {
            type Value = Secret;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string, or a table { env = \"VARIABLE\" }")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Secret, E> {
                if value.is_empty() {
                    return Err(E::custom("a secret is not empty"));
                }
                Ok(Secret::Value(Sensitive(value.to_owned())))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Secret, A::Error> {
                let FromEnv { env } =
                    FromEnv::deserialize(de::value::MapAccessDeserializer::new(map))?;
                if env.is_empty() {
                    return Err(de::Error::custom(
                        "an environment variable name is not empty",
                    ));
                }
                Ok(Secret::Env(env))
            }
        }

        deserializer.deserialize_any(SecretVisitor)
    }
}

/// A secret that is not available.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SecretError {
    /// The variable is not set, in the environment or the env file.
    #[error("${0} is not set (in the environment or --env-file)")]
    NotSet(String),
    /// The value of the variable is not Unicode.
    #[error("${0} is not Unicode")]
    NotUnicode(String),
    /// The value of the variable is empty.
    #[error("${0} is empty")]
    Empty(String),
}

/// How a server signs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Auth {
    /// An API token (official server: account settings, auth tokens).
    Token(Secret),
    /// The email and the password of the account (screepsmod-auth).
    Password {
        /// The email of the account.
        email: String,
        /// The password of the account.
        password: Secret,
    },
}

impl Auth {
    /// The credentials, with the secret read from the file, the
    /// environment, or the `--env-file` variables `env`.
    ///
    /// # Errors
    ///
    /// When the secret is not available.
    pub(crate) fn resolve(&self, env: &EnvFile) -> Result<Credentials<'_>, SecretError> {
        Ok(match self {
            Self::Token(token) => Credentials::Token(token.resolve(env)?),
            Self::Password { email, password } => Credentials::Password {
                email,
                password: password.resolve(env)?,
            },
        })
    }
}

impl fmt::Display for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token(token) => write!(f, "token {token}"),
            Self::Password { email, password } => write!(f, "{email}, password {password}"),
        }
    }
}

/// The credentials of a server, with the secret read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Credentials<'a> {
    /// An API token.
    Token(Sensitive),
    /// The email and the password of the account.
    Password {
        /// The email of the account.
        email: &'a str,
        /// The password of the account.
        password: Sensitive,
    },
}

/// A server: the URL of its API and how to sign in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "ServerFile")]
pub(crate) struct Server {
    /// The URL that the API is under.
    pub(crate) url: ServerUrl,
    /// How to sign in.
    pub(crate) auth: Auth,
}

/// A server as the file writes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerFile {
    url: ServerUrl,
    token: Option<Secret>,
    email: Option<String>,
    password: Option<Secret>,
}

impl TryFrom<ServerFile> for Server {
    type Error = AuthError;

    fn try_from(file: ServerFile) -> Result<Self, AuthError> {
        let auth = match (file.token, file.email, file.password) {
            (Some(token), None, None) => Auth::Token(token),
            (None, Some(email), Some(password)) => Auth::Password { email, password },
            (Some(_), _, _) => return Err(AuthError::Both),
            (None, _, _) => return Err(AuthError::Missing),
        };
        Ok(Self {
            url: file.url,
            auth,
        })
    }
}

/// A server without one way to sign in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum AuthError {
    /// Neither a token nor an email and a password.
    #[error("set `token`, or `email` and `password`")]
    Missing,
    /// A token and an email or a password.
    #[error("set `token`, or `email` and `password`, not both")]
    Both,
}

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

/// The shard of a spawn: `auto`, the one shard with CPU, or a name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(crate) enum ShardChoice {
    /// The only shard where the account has CPU.
    Auto,
    /// The shard of this name.
    Named(ShardName),
}

impl TryFrom<String> for ShardChoice {
    type Error = InvalidName;

    fn try_from(name: String) -> Result<Self, InvalidName> {
        if name == "auto" {
            Ok(Self::Auto)
        } else {
            ShardName::try_from(name).map(Self::Named)
        }
    }
}

impl fmt::Display for ShardChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.pad("auto"),
            Self::Named(name) => name.fmt(f),
        }
    }
}

/// How `poll` places the spawn of a profile's account.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "SpawnFile")]
pub(crate) struct Spawn {
    /// The program and arguments of the selector (no shell), which runs in
    /// the directory of the configuration file.
    pub(crate) selector: Vec<String>,
    /// The shard; none on a private server without shards.
    pub(crate) shard: Option<ShardChoice>,
    /// The time between two polls.
    pub(crate) interval: Duration,
    /// How many rooms around the start room of the server, in each
    /// direction, the candidates come from.
    pub(crate) radius: u32,
    /// The most candidate rooms that the selector chooses from.
    pub(crate) candidates: usize,
}

impl Spawn {
    /// The bounds of `interval`, in seconds: the official server allows
    /// one Memory read a minute.
    pub(crate) const INTERVAL: RangeInclusive<u64> = 60..=86_400;
    /// The bounds of `radius`.
    pub(crate) const RADIUS: RangeInclusive<u64> = 1..=10;
    /// The bounds of `candidates`.
    pub(crate) const CANDIDATES: RangeInclusive<u64> = 1..=64;
    /// The default of `interval`, in seconds.
    const DEFAULT_INTERVAL: u64 = 60;
    /// The default of `radius`.
    const DEFAULT_RADIUS: u64 = 5;
    /// The default of `candidates`.
    const DEFAULT_CANDIDATES: u64 = 16;
}

/// A spawn table as the file writes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnFile {
    selector: Vec<String>,
    shard: Option<ShardChoice>,
    interval: Option<u64>,
    radius: Option<u64>,
    candidates: Option<u64>,
}

impl TryFrom<SpawnFile> for Spawn {
    type Error = SpawnError;

    fn try_from(file: SpawnFile) -> Result<Self, SpawnError> {
        if file.selector.first().is_none_or(String::is_empty) {
            return Err(SpawnError::Selector);
        }
        let bounded = |key, value: Option<u64>, default, range: RangeInclusive<u64>| {
            let value = value.unwrap_or(default);
            if range.contains(&value) {
                Ok(value)
            } else {
                Err(SpawnError::Range { key, value, range })
            }
        };
        let interval = bounded(
            "interval",
            file.interval,
            Self::DEFAULT_INTERVAL,
            Self::INTERVAL,
        )?;
        let radius = bounded("radius", file.radius, Self::DEFAULT_RADIUS, Self::RADIUS)?;
        let candidates = bounded(
            "candidates",
            file.candidates,
            Self::DEFAULT_CANDIDATES,
            Self::CANDIDATES,
        )?;
        Ok(Self {
            selector: file.selector,
            shard: file.shard,
            interval: Duration::from_secs(interval),
            radius: u32::try_from(radius).map_err(|_| SpawnError::Range {
                key: "radius",
                value: radius,
                range: Self::RADIUS,
            })?,
            candidates: usize::try_from(candidates).map_err(|_| SpawnError::Range {
                key: "candidates",
                value: candidates,
                range: Self::CANDIDATES,
            })?,
        })
    }
}

/// A spawn table that is not valid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SpawnError {
    /// The selector has no program.
    #[error("`selector` is the program and its arguments, the program not empty")]
    Selector,
    /// A number is out of its bounds.
    #[error("`{key}` = {value}: use {}..={}", .range.start(), .range.end())]
    Range {
        /// The key.
        key: &'static str,
        /// The value.
        value: u64,
        /// The bounds.
        range: RangeInclusive<u64>,
    },
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

    /// A secret variable comes from the process, or else from the env
    /// file; empty and missing values are errors that name the variable.
    #[test]
    fn secrets() {
        let file = EnvFile::of(&[("FILE", "from-file"), ("BOTH", "from-file")]);
        let process = |name: &str| match name {
            "BOTH" => Ok("from-process".to_owned()),
            "EMPTY" => Ok(String::new()),
            _ => Err(std::env::VarError::NotPresent),
        };
        let resolve = |name: &str| {
            Secret::Env(name.to_owned())
                .resolve_with(&file, &process)
                .map(|value| value.expose().to_owned())
        };
        assert_eq!(resolve("FILE").as_deref(), Ok("from-file"));
        assert_eq!(resolve("BOTH").as_deref(), Ok("from-process"));
        assert_eq!(
            resolve("EMPTY"),
            Err(SecretError::Empty("EMPTY".to_owned()))
        );
        assert_eq!(resolve("NONE"), Err(SecretError::NotSet("NONE".to_owned())));
        let value = Secret::Value(Sensitive::new("v".to_owned()));
        assert_eq!(
            value.resolve_with(&EnvFile::default(), &process),
            Ok(Sensitive::new("v".to_owned()))
        );
    }
}
