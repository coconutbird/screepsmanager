//! The servers of the configuration: the URL that the API is under, and
//! one way to sign in to an account on it.

use std::fmt;

use serde::Deserialize;
use url::Url;

use super::secret::{Secret, SecretError, Sensitive};
use crate::envfile::EnvFile;

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
