//! The configuration file: named destinations, each a Screeps server, the
//! credentials of an account on it, and a branch. The format is the one of
//! `@coconutbird/plugin-screeps-rollup`:
//!
//! ```json
//! {
//!   "main": { "token": "TOKEN", "branch": "default" },
//!   "season": { "token": "TOKEN", "path": "/season" },
//!   "pserver": {
//!     "email": "name@server.tld", "password": "PASSWORD",
//!     "protocol": "http", "hostname": "127.0.0.1", "port": 21025,
//!     "branch": "auto"
//!   }
//! }
//! ```
//!
//! Every key is optional but the credentials: `token`, or `email` and
//! `password` (`token` wins when both are set). `protocol` is `https`,
//! `hostname` `screeps.com`, `port` the default of the protocol, `path` `/`,
//! and `branch` `default` when not set. A key that the format does not have
//! is an error, so that a misspelt key does not upload to the default server.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use reqwest::Url;
use serde::Deserialize;

/// The branch that names the current git branch.
pub(crate) const AUTO_BRANCH: &str = "auto";
/// The protocol when the destination names none.
const DEFAULT_PROTOCOL: &str = "https";
/// The host when the destination names none: the official server.
const DEFAULT_HOSTNAME: &str = "screeps.com";
/// The branch when the destination names none.
const DEFAULT_BRANCH: &str = "default";

/// A destination as the file writes it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    token: Option<String>,
    email: Option<String>,
    password: Option<String>,
    protocol: Option<String>,
    hostname: Option<String>,
    port: Option<u16>,
    path: Option<String>,
    branch: Option<String>,
}

/// How a destination signs in.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Credentials {
    /// An API token (on the official server: account settings, auth tokens).
    Token(String),
    /// The email and the password of the account, for a private server with
    /// password sign-in (screepsmod-auth).
    Password {
        /// The email of the account.
        email: String,
        /// The password of the account.
        password: String,
    },
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token(_) => f.write_str("Token(..)"),
            Self::Password { email, .. } => f
                .debug_struct("Password")
                .field("email", email)
                .finish_non_exhaustive(),
        }
    }
}

/// A destination: a server, the credentials of an account on it, and the
/// branch of the account that receives the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Destination {
    /// The base of the API paths, with a final `/`
    /// (`https://screeps.com/season/`).
    pub(crate) url: Url,
    /// How the destination signs in.
    pub(crate) credentials: Credentials,
    /// The branch, or [`AUTO_BRANCH`].
    pub(crate) branch: String,
}

/// The destination `name` of the configuration file `path`.
///
/// # Errors
///
/// When the file is not readable or does not parse, it has no destination
/// `name`, or that destination is not valid.
pub(crate) fn destination(path: &Path, name: &str) -> Result<Destination, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    parse(&text, name).map_err(|error| format!("{}: {error}", path.display()))
}

/// The destination `name` of the configuration `text`.
fn parse(text: &str, name: &str) -> Result<Destination, String> {
    let mut entries: BTreeMap<String, Entry> =
        serde_json::from_str(text).map_err(|error| error.to_string())?;
    let Some(entry) = entries.remove(name) else {
        let known: Vec<&str> = entries.keys().map(String::as_str).collect();
        let known = if known.is_empty() {
            "none".to_owned()
        } else {
            known.join(", ")
        };
        return Err(format!("no destination {name:?}; destinations: {known}"));
    };
    entry
        .destination()
        .map_err(|error| format!("destination {name:?}: {error}"))
}

impl Entry {
    /// The destination of the entry, with the defaults of the keys that it
    /// does not set. An empty string is a key that is not set.
    fn destination(self) -> Result<Destination, String> {
        let set = |value: Option<String>| value.filter(|value| !value.is_empty());
        let credentials = match (set(self.token), set(self.email), set(self.password)) {
            (Some(token), _, _) => Credentials::Token(token),
            (None, Some(email), Some(password)) => Credentials::Password { email, password },
            (None, _, _) => return Err("set `token`, or `email` and `password`".to_owned()),
        };
        let protocol = set(self.protocol);
        let protocol = protocol.as_deref().unwrap_or(DEFAULT_PROTOCOL);
        if !matches!(protocol, "http" | "https") {
            return Err(format!("protocol {protocol:?}: use http or https"));
        }
        let hostname = set(self.hostname);
        let hostname = hostname.as_deref().unwrap_or(DEFAULT_HOSTNAME);
        let port = self.port.map(|port| format!(":{port}")).unwrap_or_default();
        let path = self.path.as_deref().unwrap_or_default().trim_matches('/');
        let text = if path.is_empty() {
            format!("{protocol}://{hostname}{port}/")
        } else {
            format!("{protocol}://{hostname}{port}/{path}/")
        };
        let url = Url::parse(&text).map_err(|error| format!("server {text}: {error}"))?;
        Ok(Destination {
            url,
            credentials,
            branch: set(self.branch).unwrap_or_else(|| DEFAULT_BRANCH.to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The destination `name` of `text`, which must resolve.
    fn resolve(text: &str, name: &str) -> Destination {
        parse(text, name).expect("the destination resolves")
    }

    /// Unset keys take the defaults of the official server; `path` puts the
    /// API under it (the seasonal server), and a port that is the default
    /// of the protocol leaves the URL.
    #[test]
    fn defaults_and_paths() {
        let text = r#"{
            "main": { "token": "t" },
            "season": { "token": "t", "port": 443, "path": "/season" },
            "pserver": { "token": "t", "protocol": "http", "hostname": "127.0.0.1",
                         "port": 21025, "path": "", "branch": "auto" }
        }"#;
        let main = resolve(text, "main");
        assert_eq!(main.url.as_str(), "https://screeps.com/");
        assert_eq!(main.branch, DEFAULT_BRANCH);
        assert_eq!(main.credentials, Credentials::Token("t".to_owned()));
        let season = resolve(text, "season");
        assert_eq!(
            season.url.join("api/user/code").map(String::from),
            Ok("https://screeps.com/season/api/user/code".to_owned())
        );
        let pserver = resolve(text, "pserver");
        assert_eq!(pserver.url.as_str(), "http://127.0.0.1:21025/");
        assert_eq!(pserver.branch, AUTO_BRANCH);
    }

    /// A token wins over an email and a password; without a token both are
    /// needed, and an empty value is not set.
    #[test]
    fn credentials() {
        let text = r#"{
            "both": { "token": "t", "email": "e", "password": "p" },
            "password": { "token": "", "email": "e", "password": "p" },
            "email-only": { "email": "e" },
            "none": {}
        }"#;
        assert_eq!(
            resolve(text, "both").credentials,
            Credentials::Token("t".to_owned())
        );
        assert_eq!(
            resolve(text, "password").credentials,
            Credentials::Password {
                email: "e".to_owned(),
                password: "p".to_owned()
            }
        );
        for name in ["email-only", "none"] {
            let error = parse(text, name).expect_err(name);
            assert!(error.contains("set `token`"), "{error}");
        }
    }

    /// A misspelt key, an unknown protocol, and an unknown destination are
    /// errors; the last one names the destinations.
    #[test]
    fn invalid() {
        let error =
            parse(r#"{"main": {"token": "t", "hostName": "x"}}"#, "main").expect_err("unknown key");
        assert!(error.contains("hostName"), "{error}");
        let error = parse(r#"{"main": {"token": "t", "protocol": "ftp"}}"#, "main")
            .expect_err("unknown protocol");
        assert!(error.contains("ftp"), "{error}");
        let text = r#"{"main": {"token": "t"}, "sim": {"token": "t"}}"#;
        assert_eq!(
            parse(text, "season"),
            Err("no destination \"season\"; destinations: main, sim".to_owned())
        );
    }
}
