//! Sending a call and decoding its answer: the HTTP client without
//! redirects, URLs under the URL of the server, the token that an answer
//! rotates, and the outcome of an answer (`ok: 1`, or an `error` with
//! status 200, or an error status).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use reqwest::blocking::{Client as Http, RequestBuilder};
use reqwest::header::HeaderValue;
use reqwest::redirect::Policy;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::endpoint::Endpoint;
use super::error::Error;
use crate::config::ServerUrl;

/// The longest call: an upload of a few megabytes on a slow link.
const TIMEOUT: Duration = Duration::from_secs(60);
/// The most bytes of an answer that an error repeats.
const ERROR_TEXT: usize = 200;
/// The header of the token.
pub(super) const TOKEN: &str = "x-token";
/// The header that the API also takes the token in.
pub(super) const USERNAME: &str = "x-username";
/// The header of the time when a rate limit resets, in seconds since the
/// Unix epoch.
const RATE_LIMIT_RESET: &str = "x-ratelimit-reset";

/// The HTTP client of every call: it times out after [`TIMEOUT`], names
/// this program, and follows no redirect, so that the token goes to the
/// configured server only.
///
/// # Errors
///
/// When the HTTP client does not build.
pub(super) fn http() -> Result<Http, Error> {
    Http::builder()
        .timeout(TIMEOUT)
        .redirect(Policy::none())
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .map_err(Error::Client)
}

/// The query `pairs`, and `shard` when there is one.
pub(super) fn query<'a>(
    pairs: &[(&'static str, &'a str)],
    shard: Option<&'a str>,
) -> Vec<(&'static str, &'a str)> {
    let mut query = pairs.to_vec();
    if let Some(shard) = shard {
        query.push(("shard", shard));
    }
    query
}

/// The URL of `endpoint` with the query `query` on the server at `server`.
fn endpoint_url(
    server: &ServerUrl,
    endpoint: Endpoint,
    query: &[(&str, &str)],
) -> Result<url::Url, Error> {
    let mut url = server
        .join(endpoint.path())
        .map_err(|source| Error::Url { endpoint, source })?;
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    Ok(url)
}

/// The request of `endpoint` with the query `query` on the server at
/// `server`.
///
/// # Errors
///
/// When the URL of the call does not parse.
pub(super) fn request(
    http: &Http,
    server: &ServerUrl,
    endpoint: Endpoint,
    query: &[(&str, &str)],
) -> Result<RequestBuilder, Error> {
    Ok(http.request(endpoint.method(), endpoint_url(server, endpoint, query)?))
}

/// Sends `request` of `endpoint`, and returns the data of the answer and
/// its new token.
///
/// # Errors
///
/// When the request fails, the server limits the rate, or the answer is
/// not a success with the data of the call.
pub(super) fn call<T: DeserializeOwned>(
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
            .and_then(|reset| {
                // A clock before the epoch leaves the wait unknown.
                let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
                Some(Duration::from_secs(reset).saturating_sub(now))
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::api::{Me, WorldStatus};

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

    /// Queries are encoded under the server URL; the shard is last.
    #[test]
    fn queries() {
        let server = ServerUrl::try_from("https://screeps.com/season".to_owned()).expect("a URL");
        let url = endpoint_url(
            &server,
            Endpoint::RoomTerrain,
            &query(&[("room", "W1N1"), ("encoded", "1")], Some("shard 3&x")),
        )
        .expect("a URL");
        assert_eq!(
            url.as_str(),
            "https://screeps.com/season/api/game/room-terrain?room=W1N1&encoded=1&shard=shard+3%26x"
        );
        let bare = endpoint_url(&server, Endpoint::Me, &query(&[], None)).expect("a URL");
        assert_eq!(bare.as_str(), "https://screeps.com/season/api/auth/me");
    }

    /// The account and world answers parse; an unknown world status is an
    /// error, not an empty world.
    #[test]
    fn world_answers() {
        #[derive(Debug, Deserialize)]
        struct Status {
            status: WorldStatus,
        }

        let me: Me = reply(
            Endpoint::Me,
            StatusCode::OK,
            r#"{"ok":1,"_id":"u","username":"me","cpu":60,"cpuShard":{"shard3":60},"lastRespawnDate":1700000000000}"#.to_owned(),
        )
        .expect("auth/me parses");
        assert_eq!(
            me.cpu_shard.and_then(|cpu| cpu.get("shard3").copied()),
            Some(60.0)
        );
        assert_eq!(me.last_respawn_date, Some(1_700_000_000_000.0));

        let status =
            |text: &str| reply::<Status>(Endpoint::WorldStatus, StatusCode::OK, text.to_owned());
        assert_eq!(
            status(r#"{"ok":1,"status":"empty"}"#)
                .map(|s| s.status)
                .ok(),
            Some(WorldStatus::Empty)
        );
        assert!(matches!(
            status(r#"{"ok":1,"status":"gone"}"#),
            Err(Error::Answer { .. })
        ));
        assert!(matches!(
            reply::<Status>(Endpoint::WorldStatus, StatusCode::FOUND, String::new()),
            Err(Error::Status { .. })
        ));
    }
}
