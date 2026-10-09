//! The calls of the Screeps API that an upload makes ([`Endpoint`]). The
//! paths are under the URL of the server (`https://screeps.com/season/`):
//!
//! ```text
//! POST api/auth/signin             {"email", "password"} -> {"token"}
//! GET  api/user/branches           -> {"list": [{"branch", "activeWorld", "activeSim"}, ...]}
//! POST api/user/clone-branch       {"branch": "", "newName", "defaultModules"}
//! POST api/user/code               {"branch", "modules"}
//! POST api/user/set-active-branch  {"branch", "activeName"}
//! ```
//!
//! Every call after sign-in sends the token as `X-Token` and `X-Username`;
//! an answer with an `X-Token` header replaces the token. The server answers
//! `{"ok": 1, ...}`, or `{"error": "..."}` with status 200 when it refuses.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::blocking::{Client as Http, RequestBuilder};
use reqwest::header::{HeaderValue, InvalidHeaderValue};
use reqwest::{Method, StatusCode};
use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};

use crate::branch::{Active, BranchName};
use crate::config::{Credentials, Sensitive, ServerUrl};
use crate::modules::Modules;

/// The longest call: an upload of a few megabytes on a slow link.
const TIMEOUT: Duration = Duration::from_secs(60);
/// The most bytes of an answer that an error repeats.
const ERROR_TEXT: usize = 200;
/// The header of the token.
const TOKEN: &str = "x-token";
/// The header that the API also takes the token in.
const USERNAME: &str = "x-username";
/// The header of the time when a rate limit resets, in seconds since the
/// Unix epoch.
const RATE_LIMIT_RESET: &str = "x-ratelimit-reset";

/// A call of the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Endpoint {
    /// Signs in with an email and a password.
    SignIn,
    /// Lists the branches of the account.
    Branches,
    /// Creates a branch.
    CloneBranch,
    /// Replaces the modules of a branch.
    Code,
    /// Makes a branch run in the world or the simulator.
    SetActiveBranch,
}

impl Endpoint {
    /// The method of the call.
    fn method(self) -> Method {
        match self {
            Self::Branches => Method::GET,
            Self::SignIn | Self::CloneBranch | Self::Code | Self::SetActiveBranch => Method::POST,
        }
    }

    /// The path of the call under the URL of the server.
    fn path(self) -> &'static str {
        match self {
            Self::SignIn => "api/auth/signin",
            Self::Branches => "api/user/branches",
            Self::CloneBranch => "api/user/clone-branch",
            Self::Code => "api/user/code",
            Self::SetActiveBranch => "api/user/set-active-branch",
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.path())
    }
}

/// A branch of the account.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BranchInfo {
    /// The name, as the server has it.
    pub(crate) branch: String,
    /// Whether the branch runs in the world.
    #[serde(default)]
    pub(crate) active_world: bool,
    /// Whether the branch runs in the simulator.
    #[serde(default)]
    pub(crate) active_sim: bool,
}

impl BranchInfo {
    /// Whether the branch runs in `active`.
    pub(crate) fn runs_in(&self, active: Active) -> bool {
        match active {
            Active::World => self.active_world,
            Active::Sim => self.active_sim,
        }
    }
}

/// A client of one server, signed in.
pub(crate) struct Client {
    http: Http,
    url: ServerUrl,
    token: HeaderValue,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// A client of the server at `url`, signed in with `credentials`: a
    /// token as it is, an email and a password through [`Endpoint::SignIn`].
    ///
    /// # Errors
    ///
    /// When the HTTP client does not build, the sign-in fails, or the token
    /// is not a valid header value.
    pub(crate) fn sign_in(url: &ServerUrl, credentials: &Credentials<'_>) -> Result<Self, Error> {
        #[derive(Serialize)]
        struct SignIn<'a> {
            email: &'a str,
            password: &'a str,
        }
        #[derive(Deserialize)]
        struct SignedIn {
            token: String,
        }

        let http = Http::builder()
            .timeout(TIMEOUT)
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(Error::Client)?;
        let token = match credentials {
            Credentials::Token(token) => token.clone(),
            Credentials::Password { email, password } => {
                let body = SignIn {
                    email,
                    password: password.expose(),
                };
                let request = request(&http, url, Endpoint::SignIn)?.json(&body);
                let reply: SignedIn = call(Endpoint::SignIn, request)?.0;
                Sensitive::new(reply.token)
            }
        };
        let mut token = HeaderValue::from_str(token.expose()).map_err(Error::Token)?;
        token.set_sensitive(true);
        Ok(Self {
            http,
            url: url.clone(),
            token,
        })
    }

    /// The branches of the account.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub(crate) fn branches(&mut self) -> Result<Vec<BranchInfo>, Error> {
        #[derive(Deserialize)]
        struct Branches {
            list: Vec<BranchInfo>,
        }

        let reply: Branches = self.call(Endpoint::Branches, None::<&()>)?;
        Ok(reply.list)
    }

    /// Creates the branch `branch` with `modules`.
    ///
    /// # Errors
    ///
    /// When the call fails, for example when the account has the most
    /// branches that the server allows.
    pub(crate) fn create_branch(
        &mut self,
        branch: &BranchName,
        modules: &Modules,
    ) -> Result<(), Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct CloneBranch<'a> {
            branch: &'a str,
            new_name: &'a BranchName,
            default_modules: &'a Modules,
        }

        let body = CloneBranch {
            branch: "",
            new_name: branch,
            default_modules: modules,
        };
        self.call::<IgnoredAny>(Endpoint::CloneBranch, Some(&body))?;
        Ok(())
    }

    /// Replaces the modules of the branch `branch` with `modules`.
    ///
    /// # Errors
    ///
    /// When the call fails, for example when the branch does not exist or
    /// the code is above the size limit of the server.
    pub(crate) fn set_code(&mut self, branch: &BranchName, modules: &Modules) -> Result<(), Error> {
        #[derive(Serialize)]
        struct Code<'a> {
            branch: &'a BranchName,
            modules: &'a Modules,
        }

        self.call::<IgnoredAny>(Endpoint::Code, Some(&Code { branch, modules }))?;
        Ok(())
    }

    /// Makes the branch `branch` the one that runs in `active`.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub(crate) fn set_active_branch(
        &mut self,
        branch: &BranchName,
        active: Active,
    ) -> Result<(), Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SetActiveBranch<'a> {
            branch: &'a BranchName,
            active_name: &'static str,
        }

        let body = SetActiveBranch {
            branch,
            active_name: active.api_name(),
        };
        self.call::<IgnoredAny>(Endpoint::SetActiveBranch, Some(&body))?;
        Ok(())
    }

    /// The data of the call `endpoint` with `body` and the token, which a
    /// new token of the answer replaces.
    fn call<T: DeserializeOwned>(
        &mut self,
        endpoint: Endpoint,
        body: Option<&impl Serialize>,
    ) -> Result<T, Error> {
        let mut request = request(&self.http, &self.url, endpoint)?
            .header(TOKEN, self.token.clone())
            .header(USERNAME, self.token.clone());
        if let Some(body) = body {
            request = request.json(body);
        }
        let (data, token) = call(endpoint, request)?;
        if let Some(mut token) = token {
            token.set_sensitive(true);
            self.token = token;
        }
        Ok(data)
    }
}

/// The request of `endpoint` on the server at `url`.
fn request(http: &Http, url: &ServerUrl, endpoint: Endpoint) -> Result<RequestBuilder, Error> {
    let url = url
        .join(endpoint.path())
        .map_err(|source| Error::Url { endpoint, source })?;
    Ok(http.request(endpoint.method(), url))
}

/// Sends `request` of `endpoint`, and returns the data of the answer and
/// its new token.
fn call<T: DeserializeOwned>(
    endpoint: Endpoint,
    request: RequestBuilder,
) -> Result<(T, Option<HeaderValue>), Error> {
    let response = request
        .send()
        .map_err(|source| Error::Request { endpoint, source })?;
    let headers = response.headers();
    let token = headers
        .get(TOKEN)
        .filter(|token| !token.is_empty())
        .cloned();
    let status = response.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        let reset = headers
            .get(RATE_LIMIT_RESET)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(|reset| {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default();
                Duration::from_secs(reset).saturating_sub(now)
            });
        return Err(Error::RateLimited { endpoint, reset });
    }
    let text = response
        .text()
        .map_err(|source| Error::Request { endpoint, source })?;
    Ok((reply(endpoint, status, text)?, token))
}

/// The data of the answer to `endpoint` with the status `status` and the
/// body `text`.
fn reply<T: DeserializeOwned>(
    endpoint: Endpoint,
    status: StatusCode,
    text: String,
) -> Result<T, Error> {
    /// The fields of every answer.
    #[derive(Deserialize)]
    struct Outcome {
        ok: Option<i64>,
        error: Option<String>,
    }

    match serde_json::from_str::<Outcome>(&text).ok() {
        Some(Outcome {
            error: Some(message),
            ..
        }) => return Err(Error::Refused { endpoint, message }),
        _ if status == StatusCode::UNAUTHORIZED => return Err(Error::Unauthorized { endpoint }),
        _ if !status.is_success() => {
            return Err(Error::Status {
                endpoint,
                status,
                body: snippet(text),
            });
        }
        Some(Outcome { ok: Some(1), .. }) => {}
        _ => {
            return Err(Error::NotOk {
                endpoint,
                body: snippet(text),
            });
        }
    }
    serde_json::from_str(&text).map_err(|source| Error::Answer {
        endpoint,
        source,
        body: snippet(text),
    })
}

/// The start of `text`, for an error.
fn snippet(mut text: String) -> String {
    let mut end = text.len().min(ERROR_TEXT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

/// The text of the time until a rate limit resets.
fn reset_text(reset: Option<&Duration>) -> String {
    reset.map_or_else(String::new, |reset| {
        format!("; the limit resets in {} s", reset.as_secs())
    })
}

/// A call that failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The HTTP client does not build.
    #[error("the HTTP client: {0}")]
    Client(#[source] reqwest::Error),
    /// The token is not a valid header value.
    #[error("the token is not a valid header value")]
    Token(#[source] InvalidHeaderValue),
    /// The URL of the call does not parse.
    #[error("{endpoint}: {source}")]
    Url {
        /// The call.
        endpoint: Endpoint,
        /// Why the URL does not parse.
        source: url::ParseError,
    },
    /// The request or the answer failed.
    #[error("{endpoint}: {source}")]
    Request {
        /// The call.
        endpoint: Endpoint,
        /// What failed.
        source: reqwest::Error,
    },
    /// The server refused the call.
    #[error("{endpoint}: the server refused: {message}")]
    Refused {
        /// The call.
        endpoint: Endpoint,
        /// Why the server refused.
        message: String,
    },
    /// The credentials are not valid (status 401).
    #[error("{endpoint}: not authorized; check the credentials of the server")]
    Unauthorized {
        /// The call.
        endpoint: Endpoint,
    },
    /// The account made too many calls (status 429).
    #[error("{endpoint}: rate limited{}", reset_text(.reset.as_ref()))]
    RateLimited {
        /// The call.
        endpoint: Endpoint,
        /// The time until the limit resets, when the server says.
        reset: Option<Duration>,
    },
    /// The server answered with an error status.
    #[error("{endpoint}: status {status}: {body}")]
    Status {
        /// The call.
        endpoint: Endpoint,
        /// The status.
        status: StatusCode,
        /// The start of the answer.
        body: String,
    },
    /// The answer has no `ok: 1`.
    #[error("{endpoint}: the answer is not ok: {body}")]
    NotOk {
        /// The call.
        endpoint: Endpoint,
        /// The start of the answer.
        body: String,
    },
    /// The answer does not have the data of the call.
    #[error("{endpoint}: unexpected answer ({source}): {body}")]
    Answer {
        /// The call.
        endpoint: Endpoint,
        /// What is missing or wrong.
        source: serde_json::Error,
        /// The start of the answer.
        body: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refusal arrives with status 200; the data needs `ok: 1`; other
    /// statuses are errors with the start of the answer.
    #[test]
    fn replies() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Token {
            token: String,
        }

        let reply = |status, text: &str| reply::<Token>(Endpoint::SignIn, status, text.to_owned());
        assert_eq!(
            reply(StatusCode::OK, r#"{"ok":1,"token":"t"}"#).ok(),
            Some(Token {
                token: "t".to_owned()
            })
        );
        assert!(matches!(
            reply(StatusCode::OK, r#"{"error":"branch does not exist"}"#),
            Err(Error::Refused { message, .. }) if message == "branch does not exist"
        ));
        assert!(matches!(
            reply(StatusCode::UNAUTHORIZED, "Unauthorized"),
            Err(Error::Unauthorized { .. })
        ));
        assert!(matches!(
            reply(StatusCode::BAD_GATEWAY, "<html>"),
            Err(Error::Status { body, .. }) if body == "<html>"
        ));
        assert!(matches!(
            reply(StatusCode::OK, r#"{"token":"t"}"#),
            Err(Error::NotOk { .. })
        ));
        assert!(matches!(
            reply(StatusCode::OK, r#"{"ok":1}"#),
            Err(Error::Answer { .. })
        ));
        assert_eq!(snippet("é".repeat(150)).len(), ERROR_TEXT);
    }

    /// A branch runs where the server says; missing flags are false.
    #[test]
    fn branch_info() {
        let list: Vec<BranchInfo> = serde_json::from_str(
            r#"[{"branch":"main","activeWorld":true,"activeSim":false},{"branch":"x"}]"#,
        )
        .expect("branches parse");
        assert!(list[0].runs_in(Active::World) && !list[0].runs_in(Active::Sim));
        assert!(!list[1].runs_in(Active::World) && !list[1].runs_in(Active::Sim));
    }
}
