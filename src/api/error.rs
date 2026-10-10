//! The failures of a call, and whether a failed call may still have
//! changed the world.

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::InvalidHeaderValue;

use super::endpoint::Endpoint;

/// A call that failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The HTTP client does not build.
    #[error("the HTTP client: {0}")]
    Client(#[source] reqwest::Error),
    /// The token is not a valid header value.
    #[error("the token is not a valid header value")]
    Token(#[source] InvalidHeaderValue),
    /// A read-only client refused a call that changes the world, without
    /// sending it.
    #[error("{endpoint}: not sent: changes the world, and this run is read-only")]
    ReadOnly {
        /// The call.
        endpoint: Endpoint,
    },
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

impl Error {
    /// Whether the call surely changed nothing on the server: it was not
    /// sent, or the server refused it, the credentials, or the rate.
    /// Otherwise a call that changes the world may have happened, and
    /// only a fresh read of the world tells.
    pub(crate) fn unapplied(&self) -> bool {
        match self {
            Self::Client(_)
            | Self::Token(_)
            | Self::ReadOnly { .. }
            | Self::Url { .. }
            | Self::Refused { .. }
            | Self::Unauthorized { .. }
            | Self::RateLimited { .. } => true,
            Self::Request { source, .. } => source.is_connect() || source.is_builder(),
            Self::Status { .. } | Self::NotOk { .. } | Self::Answer { .. } => false,
        }
    }
}

/// The text of the time until a rate limit resets.
fn reset_text(reset: Option<&Duration>) -> String {
    reset.map_or_else(String::new, |reset| {
        format!("; the limit resets in {} s", reset.as_secs())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only code, branch, respawn, and spawn calls change the world;
    /// map-stats is a POST that reads.
    #[test]
    fn world_changes() {
        for endpoint in [
            Endpoint::CloneBranch,
            Endpoint::Code,
            Endpoint::SetActiveBranch,
            Endpoint::Respawn,
            Endpoint::PlaceSpawn,
        ] {
            assert!(endpoint.changes_world(), "{endpoint}");
        }
        for endpoint in [
            Endpoint::SignIn,
            Endpoint::Me,
            Endpoint::MapStats,
            Endpoint::Memory,
            Endpoint::WorldStatus,
            Endpoint::RoomObjects,
        ] {
            assert!(!endpoint.changes_world(), "{endpoint}");
        }
        assert!(
            Error::ReadOnly {
                endpoint: Endpoint::Respawn
            }
            .unapplied()
        );
        let refused = Error::Refused {
            endpoint: Endpoint::PlaceSpawn,
            message: "invalid room".to_owned(),
        };
        assert!(refused.unapplied());
        let unknown = Error::Status {
            endpoint: Endpoint::PlaceSpawn,
            status: StatusCode::BAD_GATEWAY,
            body: String::new(),
        };
        assert!(!unknown.unapplied());
    }
}
