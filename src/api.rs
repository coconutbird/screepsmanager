//! The calls of the Screeps API that an upload makes. The paths are under
//! the base URL of the destination (`https://screeps.com/season/`):
//!
//! ```text
//! POST api/auth/signin             {"email", "password"} -> {"token"}
//! GET  api/user/branches           -> {"list": [{"branch", ...}, ...]}
//! POST api/user/clone-branch       {"branch": "", "newName", "defaultModules"}
//! POST api/user/code               {"branch", "modules"}
//! POST api/user/set-active-branch  {"branch", "activeName"}
//! ```
//!
//! `modules` maps each module name to its text, or to `{"binary": BASE64}`.
//! Every call after sign-in sends the token as `X-Token` and `X-Username`;
//! an answer with an `X-Token` header replaces the token. The server answers
//! `{"ok": 1, ...}`, or `{"error": "..."}` with status 200 when it refuses.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::blocking::{Client as Http, RequestBuilder};
use reqwest::{StatusCode, Url};
use serde::de::{DeserializeOwned, IgnoredAny};
use serde::ser::SerializeMap as _;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use crate::config::Credentials;
use crate::modules::Module;

/// The longest call: an upload of a few megabytes on a slow link.
const TIMEOUT: Duration = Duration::from_secs(60);
/// The most bytes of an answer that an error repeats.
const ERROR_TEXT: usize = 200;

/// Where a branch runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Active {
    /// The world: the shards of the server.
    World,
    /// The simulator.
    Sim,
}

impl Active {
    /// The name, for output.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::World => "world",
            Self::Sim => "sim",
        }
    }

    /// The `activeName` of `set-active-branch`.
    fn api_name(self) -> &'static str {
        match self {
            Self::World => "activeWorld",
            Self::Sim => "activeSim",
        }
    }
}

/// A signed-in client of one server.
pub(crate) struct Client {
    http: Http,
    base: Url,
    token: String,
}

impl Client {
    /// A client of the server at `base`, signed in with `credentials`: a
    /// token as it is, an email and a password through `api/auth/signin`.
    ///
    /// # Errors
    ///
    /// When the HTTP client does not build or the sign-in fails.
    pub(crate) fn sign_in(base: Url, credentials: &Credentials) -> Result<Self, String> {
        let http = Http::builder()
            .timeout(TIMEOUT)
            .user_agent(concat!("screepmanager/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| format!("the HTTP client: {error}"))?;
        let mut client = Self {
            http,
            base,
            token: String::new(),
        };
        match credentials {
            Credentials::Token(token) => client.token.clone_from(token),
            Credentials::Password { email, password } => {
                #[derive(Serialize)]
                struct SignIn<'a> {
                    email: &'a str,
                    password: &'a str,
                }
                #[derive(Deserialize)]
                struct Token {
                    token: String,
                }
                let reply: Token = client.post("api/auth/signin", &SignIn { email, password })?;
                client.token = reply.token;
            }
        }
        Ok(client)
    }

    /// The names of the branches of the account.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub(crate) fn branches(&mut self) -> Result<Vec<String>, String> {
        #[derive(Deserialize)]
        struct Branches {
            list: Vec<Branch>,
        }
        #[derive(Deserialize)]
        struct Branch {
            branch: String,
        }
        let reply: Branches = self.get("api/user/branches")?;
        Ok(reply.list.into_iter().map(|item| item.branch).collect())
    }

    /// Creates the branch `branch` with `modules`.
    ///
    /// # Errors
    ///
    /// When the call fails, for example when the account has the most
    /// branches that the server allows.
    pub(crate) fn create_branch(&mut self, branch: &str, modules: &[Module]) -> Result<(), String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct CloneBranch<'a> {
            branch: &'a str,
            new_name: &'a str,
            default_modules: Modules<'a>,
        }
        let body = CloneBranch {
            branch: "",
            new_name: branch,
            default_modules: Modules(modules),
        };
        self.post::<IgnoredAny>("api/user/clone-branch", &body)
            .map(drop)
    }

    /// Replaces the modules of the branch `branch` with `modules`.
    ///
    /// # Errors
    ///
    /// When the call fails, for example when the branch does not exist or
    /// the code is above the size limit of the server.
    pub(crate) fn set_code(&mut self, branch: &str, modules: &[Module]) -> Result<(), String> {
        #[derive(Serialize)]
        struct Code<'a> {
            branch: &'a str,
            modules: Modules<'a>,
        }
        let body = Code {
            branch,
            modules: Modules(modules),
        };
        self.post::<IgnoredAny>("api/user/code", &body).map(drop)
    }

    /// Makes the branch `branch` the one that runs in `active`.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub(crate) fn set_active_branch(&mut self, branch: &str, active: Active) -> Result<(), String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SetActive<'a> {
            branch: &'a str,
            active_name: &'a str,
        }
        let body = SetActive {
            branch,
            active_name: active.api_name(),
        };
        self.post::<IgnoredAny>("api/user/set-active-branch", &body)
            .map(drop)
    }

    /// The data of a GET of `path`.
    fn get<T: DeserializeOwned>(&mut self, path: &str) -> Result<T, String> {
        let url = self.url(path)?;
        self.send(path, self.http.get(url))
    }

    /// The data of a POST of `body` to `path`.
    fn post<T: DeserializeOwned>(
        &mut self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<T, String> {
        let url = self.url(path)?;
        self.send(path, self.http.post(url).json(body))
    }

    /// The URL of the API path `path`.
    fn url(&self, path: &str) -> Result<Url, String> {
        self.base
            .join(path)
            .map_err(|error| format!("{path}: {error}"))
    }

    /// Sends `request` to `path` with the token and returns the data of the
    /// answer (see [`reply`]).
    fn send<T: DeserializeOwned>(
        &mut self,
        path: &str,
        request: RequestBuilder,
    ) -> Result<T, String> {
        let request = if self.token.is_empty() {
            request
        } else {
            request
                .header("X-Token", &self.token)
                .header("X-Username", &self.token)
        };
        let fail = |error: String| format!("{path}: {error}");
        let response = request.send().map_err(|error| fail(error.to_string()))?;
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.is_empty())
        };
        if let Some(token) = header("x-token") {
            token.clone_into(&mut self.token);
        }
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            let reset = header("x-ratelimit-reset")
                .and_then(|reset| reset.parse::<u64>().ok())
                .map(|reset| {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |now| now.as_secs());
                    format!("; the limit resets in {} s", reset.saturating_sub(now))
                })
                .unwrap_or_default();
            return Err(fail(format!("rate limited (status 429){reset}")));
        }
        let text = response.text().map_err(|error| fail(error.to_string()))?;
        reply(status, &text).map_err(fail)
    }
}

/// The data of an answer with the status `status` and the body `text`.
fn reply<T: DeserializeOwned>(status: StatusCode, text: &str) -> Result<T, String> {
    let value = serde_json::from_str::<Value>(text).ok();
    if let Some(error) = value
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(Value::as_str)
    {
        return Err(format!("the server refused: {error}"));
    }
    if status == StatusCode::UNAUTHORIZED {
        return Err("not authorized: check the credentials of the destination".to_owned());
    }
    if !status.is_success() {
        return Err(format!("status {status}: {}", snippet(text)));
    }
    let Some(value) = value.filter(|value| value.get("ok").and_then(Value::as_i64) == Some(1))
    else {
        return Err(format!("the answer is not ok: {}", snippet(text)));
    };
    serde_json::from_value(value).map_err(|error| format!("unexpected answer: {error}"))
}

/// The start of `text`, for an error.
fn snippet(text: &str) -> &str {
    let mut end = text.len().min(ERROR_TEXT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The `modules` of a request: each module name to its text, or to
/// `{"binary": BASE64}`.
struct Modules<'a>(&'a [Module]);

impl Serialize for Modules<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Binary<'a> {
            binary: &'a str,
        }
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for module in self.0 {
            if module.kind.is_binary() {
                map.serialize_entry(
                    &module.name,
                    &Binary {
                        binary: &module.code,
                    },
                )?;
            } else {
                map.serialize_entry(&module.name, &module.code)?;
            }
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::modules::Kind;

    /// A refusal arrives with status 200; the data needs `ok: 1`; other
    /// statuses are errors with the start of the answer.
    #[test]
    fn replies() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Token {
            token: String,
        }
        assert_eq!(
            reply(StatusCode::OK, r#"{"ok":1,"token":"t"}"#),
            Ok(Token {
                token: "t".to_owned()
            })
        );
        assert_eq!(
            reply::<IgnoredAny>(StatusCode::OK, r#"{"error":"branch does not exist"}"#).map(drop),
            Err("the server refused: branch does not exist".to_owned())
        );
        for (status, text) in [
            (StatusCode::OK, r#"{"token":"t"}"#),
            (StatusCode::OK, "<html>"),
            (StatusCode::BAD_GATEWAY, "<html>"),
            (StatusCode::UNAUTHORIZED, "Unauthorized"),
        ] {
            assert!(reply::<Token>(status, text).is_err(), "{status} {text}");
        }
        assert_eq!(snippet(&"é".repeat(150)).len(), ERROR_TEXT);
    }

    /// Text modules are strings; binary modules are `{"binary": ...}`.
    #[test]
    fn module_bodies() {
        let module = |name: &str, kind, code: &str| Module {
            name: name.to_owned(),
            kind,
            code: code.to_owned(),
            path: PathBuf::new(),
            len: 0,
        };
        let modules = [
            module("main", Kind::Js, "loop"),
            module("main.js.map", Kind::SourceMap, "map"),
            module("main_bg", Kind::Wasm, "AGFzbQ=="),
        ];
        assert_eq!(
            serde_json::to_string(&Modules(&modules)).map_err(|error| error.to_string()),
            Ok(r#"{"main":"loop","main.js.map":"map","main_bg":{"binary":"AGFzbQ=="}}"#.to_owned())
        );
    }
}
