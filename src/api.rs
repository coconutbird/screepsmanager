//! The calls of the Screeps API that `upload` and `poll` make
//! ([`Endpoint`]). The paths are under the URL of the server
//! (`https://screeps.com/season/`):
//!
//! ```text
//! POST api/auth/signin                   {"email", "password"} -> {"token"}
//! GET  api/auth/me                       -> {"username", "cpu", "cpuShard", "lastRespawnDate"}
//! GET  api/user/branches                 -> {"list": [{"branch", "activeWorld", "activeSim"}, ...]}
//! POST api/user/clone-branch             {"branch": "", "newName", "defaultModules"}
//! POST api/user/code                     {"branch", "modules"}
//! POST api/user/set-active-branch        {"branch", "activeName"}
//! GET  api/game/shards/info              -> {"shards": [{"name"}, ...]}
//! GET  api/game/time?shard               -> {"time"}
//! GET  api/user/world-status?shard       -> {"status": "normal" | "lost" | "empty"}
//! GET  api/user/world-start-room?shard   -> {"room": ["W1N1"]}
//! GET  api/user/respawn-prohibited-rooms?shard -> {"rooms": ["W1N1", ...]}
//! GET  api/user/memory?path&shard        -> {"data": "gz:" + base64(gzip(JSON))}
//! POST api/game/map-stats                {"rooms", "statName": "owner0", "shard"} -> {"stats": {ROOM: {...}}}
//! GET  api/game/room-status?room&shard   -> {"room": {"status", "novice", "respawnArea", "openTime"}}
//! GET  api/game/room-terrain?room&encoded=1&shard -> {"terrain": [{"room", "terrain"}]}
//! GET  api/game/room-objects?room&shard  -> {"objects": [...], "users": {...}}
//! POST api/user/respawn                  {}
//! POST api/game/place-spawn              {"room", "name", "x", "y", "shard"}
//! ```
//!
//! Every call after sign-in sends the token as `X-Token` and `X-Username`;
//! an answer with an `X-Token` header replaces the token. The server answers
//! `{"ok": 1, ...}`, or `{"error": "..."}` with status 200 when it refuses.
//! The client follows no redirect, so that the token goes to the configured
//! server only, and a read-only client refuses every call that changes the
//! account or the world ([`Endpoint::changes_world`]) before it sends it.
//!
//! The calls follow the official Screeps World HTTP contract of
//! <https://github.com/openscreeps/openapi>, which `contract/openapi.json`
//! pins (`contract/NOTICE`): the tests send every call to a fake server and
//! check each request against the operation of the contract, and list where
//! the client knowingly differs from it. The client decodes only the fields
//! that the manager reads, so that a private server that omits the others
//! or types them differently (see the standalone profile of the contract)
//! still works.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::blocking::{Client as Http, RequestBuilder};
use reqwest::header::{HeaderValue, InvalidHeaderValue};
use reqwest::redirect::Policy;
use reqwest::{Method, StatusCode};
use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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
    /// The account: its name, CPU, and last respawn.
    Me,
    /// Lists the branches of the account.
    Branches,
    /// Creates a branch.
    CloneBranch,
    /// Replaces the modules of a branch.
    Code,
    /// Makes a branch run in the world or the simulator.
    SetActiveBranch,
    /// The shards of the server.
    Shards,
    /// The game time of a shard.
    Time,
    /// Whether the account has a spawn, other objects, or nothing.
    WorldStatus,
    /// The room that the server suggests to start in.
    WorldStartRoom,
    /// The rooms where the account may not place its spawn.
    RespawnProhibitedRooms,
    /// A path of the Memory of the account.
    Memory,
    /// The owners and statuses of rooms (read-only, by POST).
    MapStats,
    /// The status of a room.
    RoomStatus,
    /// The terrain of a room.
    RoomTerrain,
    /// The objects of a room.
    RoomObjects,
    /// Removes every object of the account.
    Respawn,
    /// Places the first spawn of the account.
    PlaceSpawn,
}

impl Endpoint {
    /// The method of the call.
    fn method(self) -> Method {
        match self {
            Self::Me
            | Self::Branches
            | Self::Shards
            | Self::Time
            | Self::WorldStatus
            | Self::WorldStartRoom
            | Self::RespawnProhibitedRooms
            | Self::Memory
            | Self::RoomStatus
            | Self::RoomTerrain
            | Self::RoomObjects => Method::GET,
            Self::SignIn
            | Self::CloneBranch
            | Self::Code
            | Self::SetActiveBranch
            | Self::MapStats
            | Self::Respawn
            | Self::PlaceSpawn => Method::POST,
        }
    }

    /// The path of the call under the URL of the server.
    fn path(self) -> &'static str {
        match self {
            Self::SignIn => "api/auth/signin",
            Self::Me => "api/auth/me",
            Self::Branches => "api/user/branches",
            Self::CloneBranch => "api/user/clone-branch",
            Self::Code => "api/user/code",
            Self::SetActiveBranch => "api/user/set-active-branch",
            Self::Shards => "api/game/shards/info",
            Self::Time => "api/game/time",
            Self::WorldStatus => "api/user/world-status",
            Self::WorldStartRoom => "api/user/world-start-room",
            Self::RespawnProhibitedRooms => "api/user/respawn-prohibited-rooms",
            Self::Memory => "api/user/memory",
            Self::MapStats => "api/game/map-stats",
            Self::RoomStatus => "api/game/room-status",
            Self::RoomTerrain => "api/game/room-terrain",
            Self::RoomObjects => "api/game/room-objects",
            Self::Respawn => "api/user/respawn",
            Self::PlaceSpawn => "api/game/place-spawn",
        }
    }

    /// Whether the call changes the code, the branches, or the objects of
    /// the account: what a read-only client refuses.
    pub(crate) fn changes_world(self) -> bool {
        match self {
            Self::CloneBranch
            | Self::Code
            | Self::SetActiveBranch
            | Self::Respawn
            | Self::PlaceSpawn => true,
            Self::SignIn
            | Self::Me
            | Self::Branches
            | Self::Shards
            | Self::Time
            | Self::WorldStatus
            | Self::WorldStartRoom
            | Self::RespawnProhibitedRooms
            | Self::Memory
            | Self::MapStats
            | Self::RoomStatus
            | Self::RoomTerrain
            | Self::RoomObjects => false,
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

/// The account, as `auth/me` has it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Me {
    /// The name of the account.
    pub(crate) username: String,
    /// The CPU of the account: of every shard together on a server with
    /// shards.
    pub(crate) cpu: f64,
    /// The CPU of each shard, on a server with shards.
    #[serde(default)]
    pub(crate) cpu_shard: Option<BTreeMap<String, f64>>,
    /// When the account last respawned, in milliseconds since the Unix
    /// epoch.
    #[serde(default)]
    pub(crate) last_respawn_date: Option<f64>,
}

/// What the account has in the world of a shard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum WorldStatus {
    /// A spawn.
    Normal,
    /// Objects, but no spawn.
    Lost,
    /// No object.
    Empty,
}

impl fmt::Display for WorldStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Normal => "normal",
            Self::Lost => "lost",
            Self::Empty => "empty",
        })
    }
}

/// The spawn that [`Client::place_spawn`] places.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PlaceSpawn<'a> {
    /// The room.
    pub(crate) room: &'a str,
    /// The name of the spawn.
    pub(crate) name: &'a str,
    /// The column, 0 to 49.
    pub(crate) x: u8,
    /// The row, 0 to 49.
    pub(crate) y: u8,
    /// The shard, on a server with shards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) shard: Option<&'a str>,
}

/// What a client may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// Only calls that do not change the world ([`Endpoint::changes_world`]).
    ReadOnly,
    /// Every call.
    ReadWrite,
}

/// A client of one server, signed in.
pub(crate) struct Client {
    http: Http,
    url: ServerUrl,
    token: HeaderValue,
    access: Access,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("url", &self.url)
            .field("access", &self.access)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// A client of the server at `url`, signed in with `credentials`: a
    /// token as it is, an email and a password through [`Endpoint::SignIn`].
    /// With [`Access::ReadOnly`], it refuses the calls that change the
    /// world.
    ///
    /// # Errors
    ///
    /// When the HTTP client does not build, the sign-in fails, or the token
    /// is not a valid header value.
    pub(crate) fn sign_in(
        url: &ServerUrl,
        credentials: &Credentials<'_>,
        access: Access,
    ) -> Result<Self, Error> {
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
            .redirect(Policy::none())
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
                let request = request(&http, url, Endpoint::SignIn, &[])?.json(&body);
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
            access,
        })
    }

    /// The account.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub(crate) fn me(&mut self) -> Result<Me, Error> {
        self.call(Endpoint::Me, &[], None::<&()>)
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

        let reply: Branches = self.call(Endpoint::Branches, &[], None::<&()>)?;
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
        self.call::<IgnoredAny>(Endpoint::CloneBranch, &[], Some(&body))?;
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

        self.call::<IgnoredAny>(Endpoint::Code, &[], Some(&Code { branch, modules }))?;
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
        self.call::<IgnoredAny>(Endpoint::SetActiveBranch, &[], Some(&body))?;
        Ok(())
    }

    /// The names of the shards of the server.
    ///
    /// # Errors
    ///
    /// When the call fails, for example on a server without shards.
    pub(crate) fn shards(&mut self) -> Result<Vec<String>, Error> {
        #[derive(Deserialize)]
        struct Shard {
            name: String,
        }
        #[derive(Deserialize)]
        struct Shards {
            shards: Vec<Shard>,
        }

        let reply: Shards = self.call(Endpoint::Shards, &[], None::<&()>)?;
        Ok(reply.shards.into_iter().map(|shard| shard.name).collect())
    }

    /// The game time of `shard`.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub(crate) fn time(&mut self, shard: Option<&str>) -> Result<u64, Error> {
        #[derive(Deserialize)]
        struct Time {
            time: u64,
        }

        let reply: Time = self.call(Endpoint::Time, &query(&[], shard), None::<&()>)?;
        Ok(reply.time)
    }

    /// What the account has in the world of `shard`.
    ///
    /// # Errors
    ///
    /// When the call fails, or the status is not one that this client
    /// knows.
    pub(crate) fn world_status(&mut self, shard: Option<&str>) -> Result<WorldStatus, Error> {
        #[derive(Deserialize)]
        struct Status {
            status: WorldStatus,
        }

        let reply: Status = self.call(Endpoint::WorldStatus, &query(&[], shard), None::<&()>)?;
        Ok(reply.status)
    }

    /// The rooms that the server suggests to start in on `shard`, as it
    /// writes them (some servers prefix `shard/`).
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub(crate) fn world_start_room(&mut self, shard: Option<&str>) -> Result<Vec<String>, Error> {
        #[derive(Deserialize)]
        struct Start {
            room: Vec<String>,
        }

        let reply: Start = self.call(Endpoint::WorldStartRoom, &query(&[], shard), None::<&()>)?;
        Ok(reply.room)
    }

    /// The rooms of `shard` where the account may not place its spawn, as
    /// the server writes them.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub(crate) fn respawn_prohibited_rooms(
        &mut self,
        shard: Option<&str>,
    ) -> Result<Vec<String>, Error> {
        #[derive(Deserialize)]
        struct Rooms {
            rooms: Vec<String>,
        }

        let reply: Rooms = self.call(
            Endpoint::RespawnProhibitedRooms,
            &query(&[], shard),
            None::<&()>,
        )?;
        Ok(reply.rooms)
    }

    /// The Memory of the account at `path` on `shard` as the server encodes
    /// it (`gz:` and base64 of gzip of JSON), or none when it has no data.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub(crate) fn memory(
        &mut self,
        path: &str,
        shard: Option<&str>,
    ) -> Result<Option<String>, Error> {
        #[derive(Deserialize)]
        struct Memory {
            #[serde(default)]
            data: Option<String>,
        }

        let reply: Memory = self.call(
            Endpoint::Memory,
            &query(&[("path", path)], shard),
            None::<&()>,
        )?;
        Ok(reply.data)
    }

    /// The owner statistics (`owner0`) of `rooms` on `shard`: per room its
    /// status, owner or reservation (`own`), and novice and respawn areas.
    /// The call is a POST that changes nothing.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub(crate) fn map_stats(
        &mut self,
        rooms: &[String],
        shard: Option<&str>,
    ) -> Result<BTreeMap<String, Value>, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct MapStats<'a> {
            rooms: &'a [String],
            stat_name: &'static str,
            #[serde(skip_serializing_if = "Option::is_none")]
            shard: Option<&'a str>,
        }
        #[derive(Deserialize)]
        struct Stats {
            stats: BTreeMap<String, Value>,
        }

        let body = MapStats {
            rooms,
            stat_name: "owner0",
            shard,
        };
        let reply: Stats = self.call(Endpoint::MapStats, &[], Some(&body))?;
        Ok(reply.stats)
    }

    /// The status fields of `room` on `shard`, or none when the server has
    /// no such room.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub(crate) fn room_status(
        &mut self,
        room: &str,
        shard: Option<&str>,
    ) -> Result<Option<Map<String, Value>>, Error> {
        #[derive(Deserialize)]
        struct Status {
            #[serde(default)]
            room: Option<Map<String, Value>>,
        }

        let reply: Status = self.call(
            Endpoint::RoomStatus,
            &query(&[("room", room)], shard),
            None::<&()>,
        )?;
        Ok(reply.room)
    }

    /// The encoded terrain of `room` on `shard` (2500 digits, row by row),
    /// or none when the answer has none for the room.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub(crate) fn room_terrain(
        &mut self,
        room: &str,
        shard: Option<&str>,
    ) -> Result<Option<String>, Error> {
        #[derive(Deserialize)]
        struct Entry {
            #[serde(default)]
            room: Option<String>,
            terrain: String,
        }
        #[derive(Deserialize)]
        struct Terrain {
            terrain: Vec<Entry>,
        }

        let reply: Terrain = self.call(
            Endpoint::RoomTerrain,
            &query(&[("room", room), ("encoded", "1")], shard),
            None::<&()>,
        )?;
        Ok(reply
            .terrain
            .into_iter()
            .find(|entry| entry.room.as_deref().is_none_or(|name| name == room))
            .map(|entry| entry.terrain))
    }

    /// The objects of `room` on `shard`, as the server has them.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub(crate) fn room_objects(
        &mut self,
        room: &str,
        shard: Option<&str>,
    ) -> Result<Vec<Value>, Error> {
        #[derive(Deserialize)]
        struct Objects {
            objects: Vec<Value>,
        }

        let reply: Objects = self.call(
            Endpoint::RoomObjects,
            &query(&[("room", room)], shard),
            None::<&()>,
        )?;
        Ok(reply.objects)
    }

    /// Removes every object of the account on every shard, so that it can
    /// place a spawn again.
    ///
    /// # Errors
    ///
    /// When the call fails; [`Error::unapplied`] says whether the account
    /// may have respawned anyway.
    pub(crate) fn respawn(&mut self) -> Result<(), Error> {
        self.call::<IgnoredAny>(Endpoint::Respawn, &[], Some(&Map::new()))?;
        Ok(())
    }

    /// Places the first spawn of the account.
    ///
    /// # Errors
    ///
    /// When the call fails; [`Error::unapplied`] says whether the spawn may
    /// have been placed anyway.
    pub(crate) fn place_spawn(&mut self, spawn: &PlaceSpawn<'_>) -> Result<(), Error> {
        self.call::<IgnoredAny>(Endpoint::PlaceSpawn, &[], Some(spawn))?;
        Ok(())
    }

    /// The data of the call `endpoint` with the query `query`, `body`, and
    /// the token, which a new token of the answer replaces.
    fn call<T: DeserializeOwned>(
        &mut self,
        endpoint: Endpoint,
        query: &[(&str, &str)],
        body: Option<&impl Serialize>,
    ) -> Result<T, Error> {
        if self.access == Access::ReadOnly && endpoint.changes_world() {
            return Err(Error::ReadOnly { endpoint });
        }
        let mut request = request(&self.http, &self.url, endpoint, query)?
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

/// The query `pairs`, and `shard` when there is one.
fn query<'a>(
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
fn request(
    http: &Http,
    server: &ServerUrl,
    endpoint: Endpoint,
    query: &[(&str, &str)],
) -> Result<RequestBuilder, Error> {
    Ok(http.request(endpoint.method(), endpoint_url(server, endpoint, query)?))
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;
    use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use serde_json::json;

    /// The official contract that the client follows: `openapi.json` of
    /// <https://github.com/openscreeps/openapi> at commit
    /// `1cf3725ab8df0b61028ae3177750217430abb818`, byte for byte (blob
    /// `784aa003218a00d62b14f6bf46b8d375f6aabacb`; ISC License, see
    /// `contract/NOTICE`).
    const CONTRACT: &str = include_str!("../contract/openapi.json");

    /// Every endpoint, for the contract test.
    const ENDPOINTS: [Endpoint; 18] = [
        Endpoint::SignIn,
        Endpoint::Me,
        Endpoint::Branches,
        Endpoint::CloneBranch,
        Endpoint::Code,
        Endpoint::SetActiveBranch,
        Endpoint::Shards,
        Endpoint::Time,
        Endpoint::WorldStatus,
        Endpoint::WorldStartRoom,
        Endpoint::RespawnProhibitedRooms,
        Endpoint::Memory,
        Endpoint::MapStats,
        Endpoint::RoomStatus,
        Endpoint::RoomTerrain,
        Endpoint::RoomObjects,
        Endpoint::Respawn,
        Endpoint::PlaceSpawn,
    ];

    /// Query parameters that the client sends although the contract
    /// declares none of that name: `poll` reads the world status and the
    /// prohibited rooms per shard; the official server answers for the
    /// whole account, and a standalone server serves one world and ignores
    /// `shard`.
    const UNDECLARED_QUERY: [(&str, &str); 2] = [
        ("userWorldStatus", "shard"),
        ("userRespawnProhibitedRooms", "shard"),
    ];

    /// Operations without a request body in the contract to which the
    /// client sends an empty JSON object.
    const UNDECLARED_EMPTY_BODY: [&str; 1] = ["userRespawn"];

    /// The `operationId` of `endpoint` in the contract.
    fn operation_id(endpoint: Endpoint) -> &'static str {
        match endpoint {
            Endpoint::SignIn => "authSignin",
            Endpoint::Me => "authMe",
            Endpoint::Branches => "userBranches",
            Endpoint::CloneBranch => "userCloneBranch",
            Endpoint::Code => "userCodeSet",
            Endpoint::SetActiveBranch => "userSetActiveBranch",
            Endpoint::Shards => "gameShardsInfo",
            Endpoint::Time => "gameTime",
            Endpoint::WorldStatus => "userWorldStatus",
            Endpoint::WorldStartRoom => "userWorldStartRoom",
            Endpoint::RespawnProhibitedRooms => "userRespawnProhibitedRooms",
            Endpoint::Memory => "userMemoryGet",
            Endpoint::MapStats => "gameMapStats",
            Endpoint::RoomStatus => "gameRoomStatus",
            Endpoint::RoomTerrain => "gameRoomTerrain",
            Endpoint::RoomObjects => "gameRoomObjects",
            Endpoint::Respawn => "userRespawn",
            Endpoint::PlaceSpawn => "gamePlaceSpawn",
        }
    }

    /// The parsed contract.
    struct Contract(Value);

    /// The declared and the required properties of an object schema.
    #[derive(Default)]
    struct Shape<'a> {
        /// The declared properties.
        properties: BTreeSet<&'a str>,
        /// The required properties.
        required: BTreeSet<&'a str>,
    }

    impl Contract {
        /// The contract of `contract/openapi.json`.
        fn load() -> Self {
            Self(serde_json::from_str(CONTRACT).expect("the contract parses"))
        }

        /// `value`, or what its `$ref` points to.
        fn resolve<'a>(&'a self, mut value: &'a Value) -> &'a Value {
            while let Some(reference) = value.get("$ref").and_then(Value::as_str) {
                let pointer = reference.strip_prefix('#').expect("a local reference");
                value = self.0.pointer(pointer).expect("the reference resolves");
            }
            value
        }

        /// The operation of `method` on `path` (`/api/auth/me`).
        fn operation(&self, method: &str, path: &str) -> Option<&Value> {
            self.0
                .get("paths")?
                .get(path)?
                .get(method.to_ascii_lowercase().as_str())
        }

        /// The query parameters of `operation`, and whether each is
        /// required.
        fn query<'a>(&'a self, operation: &'a Value) -> Vec<(&'a str, bool)> {
            operation
                .get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|parameter| self.resolve(parameter))
                .filter(|parameter| parameter["in"] == "query")
                .filter_map(|parameter| {
                    let required = parameter["required"].as_bool().unwrap_or(false);
                    Some((parameter["name"].as_str()?, required))
                })
                .collect()
        }

        /// Adds the properties of the object schema `schema`, and of its
        /// `allOf` parts, to `shape`.
        fn object<'a>(&'a self, schema: &'a Value, shape: &mut Shape<'a>) {
            let schema = self.resolve(schema);
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                shape
                    .properties
                    .extend(properties.keys().map(String::as_str));
            }
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                shape
                    .required
                    .extend(required.iter().filter_map(Value::as_str));
            }
            for part in schema
                .get("allOf")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                self.object(part, shape);
            }
        }

        /// Whether every way to call `operation` needs the token.
        fn needs_token(operation: &Value) -> bool {
            operation["security"].as_array().is_some_and(|options| {
                !options.is_empty() && options.iter().all(|option| option.get("Token").is_some())
            })
        }
    }

    /// A request that a fake server received.
    #[derive(Debug, Clone)]
    struct Received {
        /// The method.
        method: String,
        /// The path and the query.
        target: String,
        /// The headers, by lowercase name.
        headers: BTreeMap<String, String>,
        /// The body.
        body: Vec<u8>,
    }

    /// The answer of a fake server.
    struct Answer {
        /// The status.
        status: u16,
        /// The headers besides the type and the length of the body.
        headers: Vec<(&'static str, String)>,
        /// The body.
        body: String,
    }

    impl Answer {
        /// A JSON answer with status 200.
        fn json(body: &Value) -> Self {
            Self {
                status: 200,
                headers: Vec::new(),
                body: body.to_string(),
            }
        }
    }

    /// A fake server on 127.0.0.1 that keeps each request and answers it,
    /// one request per connection.
    struct Fake {
        /// Its URL, with the path of the server.
        url: String,
        /// The requests, in order.
        received: Arc<Mutex<Vec<Received>>>,
        /// The listener address used to wake it during shutdown.
        address: SocketAddr,
        /// Signals the worker to stop accepting requests.
        stopped: Arc<AtomicBool>,
        /// Joined when this server leaves scope.
        worker: Option<thread::JoinHandle<()>>,
    }

    impl Fake {
        /// A fake server under `path` (`/season/`) that answers with
        /// `answer`.
        fn start(path: &str, answer: impl Fn(&Received) -> Answer + Send + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
            let address = listener.local_addr().expect("an address");
            let url = format!("http://{address}{path}");
            let received = Arc::new(Mutex::new(Vec::new()));
            let log = Arc::clone(&received);
            let stopped = Arc::new(AtomicBool::new(false));
            let stopping = Arc::clone(&stopped);
            let worker = thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    if stopping.load(Ordering::Acquire) {
                        break;
                    }
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .expect("a read timeout");
                    stream
                        .set_write_timeout(Some(Duration::from_secs(5)))
                        .expect("a write timeout");
                    if let Some(request) = read_request(&stream) {
                        let reply = answer(&request);
                        log.lock().expect("the log").push(request);
                        // A client that went away is the client's failure.
                        let _ = write_answer(&stream, &reply);
                    }
                }
            });
            Self {
                url,
                received,
                address,
                stopped,
                worker: Some(worker),
            }
        }

        /// Its URL as a server URL.
        fn server(&self) -> ServerUrl {
            ServerUrl::try_from(self.url.clone()).expect("a URL")
        }

        /// The requests so far.
        fn received(&self) -> Vec<Received> {
            self.received.lock().expect("the log").clone()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.stopped.store(true, Ordering::Release);
            let _ = TcpStream::connect(self.address);
            if let Some(worker) = self.worker.take() {
                worker.join().expect("the server worker finishes");
            }
        }
    }

    /// The request on `stream`, or none when it ends first.
    fn read_request(stream: &TcpStream) -> Option<Received> {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let mut start = line.split_whitespace();
        let method = start.next()?.to_owned();
        let target = start.next()?.to_owned();
        let mut headers = BTreeMap::new();
        loop {
            line.clear();
            reader.read_line(&mut line).ok()?;
            let header = line.trim_end();
            if header.is_empty() {
                break;
            }
            let (name, value) = header.split_once(':')?;
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
        let length = headers
            .get("content-length")
            .and_then(|length| length.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0; length];
        reader.read_exact(&mut body).ok()?;
        Some(Received {
            method,
            target,
            headers,
            body,
        })
    }

    /// Writes `answer` to `stream`, and closes the connection.
    fn write_answer(mut stream: &TcpStream, answer: &Answer) -> io::Result<()> {
        let mut head = format!(
            "HTTP/1.1 {} Fake\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
            answer.status,
            answer.body.len()
        );
        for (name, value) in &answer.headers {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        stream.write_all(answer.body.as_bytes())
    }

    /// A client of `server` with the token `t`.
    fn token_client(server: &ServerUrl, access: Access) -> Client {
        // reqwest takes TLS from the process-wide provider, which `main`
        // installs.
        let _ = rustls::crypto::ring::default_provider().install_default();
        Client::sign_in(
            server,
            &Credentials::Token(Sensitive::new("t".to_owned())),
            access,
        )
        .expect("a client")
    }

    /// The success answer to `request` with what the contract requires of
    /// it; `auth/me` also rotates the token.
    fn contract_answer(request: &Received) -> Answer {
        let path = request.target.split('?').next().unwrap_or_default();
        let path = path.strip_prefix("/season/").unwrap_or(path);
        let body = match path {
            "api/auth/signin" => json!({"ok": 1, "token": "signed"}),
            "api/auth/me" => {
                let me = json!({
                    "ok": 1, "_id": "u1", "username": "me", "cpu": 20,
                    "cpuShard": {"shard3": 20}, "lastRespawnDate": 1_700_000_000_000_u64,
                    "money": 0, "powerExperimentations": 0,
                });
                let mut answer = Answer::json(&me);
                answer.headers.push(("x-token", "rotated".to_owned()));
                return answer;
            }
            "api/user/branches" => json!({
                "ok": 1,
                "list": [{"_id": "b1", "branch": "main", "activeWorld": true, "activeSim": false}],
            }),
            "api/user/clone-branch" | "api/user/code" => {
                json!({"ok": 1, "timestamp": 1_700_000_000_000_u64})
            }
            "api/game/shards/info" => json!({
                "ok": 1,
                "shards": [{"name": "shard3", "rooms": 100, "users": 10, "tick": 4000}],
            }),
            "api/game/time" => json!({"ok": 1, "time": 123}),
            "api/user/world-status" => json!({"ok": 1, "status": "empty"}),
            "api/user/world-start-room" => json!({"ok": 1, "room": ["shard3/W5N5"]}),
            "api/user/respawn-prohibited-rooms" => json!({"ok": 1, "rooms": ["W1N1"]}),
            "api/user/memory" => json!({"ok": 1, "data": "gz:AAAA"}),
            "api/game/map-stats" => json!({
                "ok": 1, "gameTime": 123,
                "stats": {"W5N5": {"status": "normal", "novice": null, "own": {"user": "u2", "level": 3}}},
                "statsMax": {}, "users": {"u2": {"_id": "u2", "username": "other"}},
            }),
            "api/game/room-status" => json!({
                "ok": 1,
                "room": {"status": "normal", "novice": null, "respawnArea": null, "openTime": null},
            }),
            "api/game/room-terrain" => json!({
                "ok": 1,
                "terrain": [{"_id": "t1", "room": "W5N5", "terrain": "0".repeat(2500), "type": "terrain"}],
            }),
            "api/game/room-objects" => json!({
                "ok": 1,
                "objects": [{"_id": "o1", "type": "source", "room": "W5N5", "x": 10, "y": 20}],
                "users": {},
            }),
            "api/game/place-spawn" => json!({"ok": 1, "newbie": true}),
            _ => json!({"ok": 1}),
        };
        Answer::json(&body)
    }

    /// Calls every endpoint with a read-write client of `server`, signed
    /// in with a password, and checks the data of the answers.
    fn call_every_endpoint(server: &ServerUrl) -> Client {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let credentials = Credentials::Password {
            email: "me@example.com",
            password: Sensitive::new("secret".to_owned()),
        };
        let mut client =
            Client::sign_in(server, &credentials, Access::ReadWrite).expect("signed in");
        let me = client.me().expect("auth/me");
        assert_eq!((me.username.as_str(), me.cpu), ("me", 20.0));
        let branch: BranchName = "main".parse().expect("a branch");
        let modules = Modules::default();
        let branches = client.branches().expect("branches");
        assert!(branches[0].runs_in(Active::World));
        client
            .create_branch(&branch, &modules)
            .expect("clone-branch");
        client.set_code(&branch, &modules).expect("code");
        client
            .set_active_branch(&branch, Active::World)
            .expect("set-active-branch");
        assert_eq!(client.shards().expect("shards"), ["shard3"]);
        let shard = Some("shard3");
        assert_eq!(client.time(shard).expect("time"), 123);
        assert_eq!(
            client.world_status(shard).expect("world-status"),
            WorldStatus::Empty
        );
        assert_eq!(
            client.world_start_room(shard).expect("world-start-room"),
            ["shard3/W5N5"]
        );
        assert_eq!(
            client
                .respawn_prohibited_rooms(shard)
                .expect("respawn-prohibited-rooms"),
            ["W1N1"]
        );
        assert_eq!(
            client
                .memory("heartbeat", shard)
                .expect("memory")
                .as_deref(),
            Some("gz:AAAA")
        );
        let rooms = ["W5N5".to_owned()];
        let ownership = client.map_stats(&rooms, shard).expect("map-stats");
        assert!(ownership.contains_key("W5N5"));
        let status = client
            .room_status("W5N5", shard)
            .expect("room-status")
            .expect("a room status");
        assert_eq!(status.get("status").and_then(Value::as_str), Some("normal"));
        let terrain = client
            .room_terrain("W5N5", shard)
            .expect("room-terrain")
            .expect("the terrain");
        assert_eq!(terrain.len(), 2500);
        let objects = client.room_objects("W5N5", shard).expect("room-objects");
        assert_eq!(objects.len(), 1);
        client.respawn().expect("respawn");
        let spawn = PlaceSpawn {
            room: "W5N5",
            name: "Spawn1",
            x: 25,
            y: 25,
            shard,
        };
        client.place_spawn(&spawn).expect("place-spawn");
        client
    }

    /// Checks `request` against its operation in `contract`, for a server
    /// under `/season/`, and returns its endpoint.
    fn check_request(contract: &Contract, request: &Received) -> Endpoint {
        let url = url::Url::parse(&format!("http://fake{}", request.target)).expect("a target");
        let path = url
            .path()
            .strip_prefix("/season/")
            .expect("under the path of the server");
        let endpoint = ENDPOINTS
            .into_iter()
            .find(|endpoint| {
                endpoint.path() == path && endpoint.method().as_str() == request.method
            })
            .unwrap_or_else(|| panic!("{} {path}: no endpoint", request.method));
        let id = operation_id(endpoint);
        let operation = contract
            .operation(&request.method, &format!("/{path}"))
            .unwrap_or_else(|| panic!("{endpoint}: not in the contract"));
        assert_eq!(operation["operationId"], id, "{endpoint}");

        let declared = contract.query(operation);
        let sent: Vec<String> = url
            .query_pairs()
            .map(|(name, _)| name.into_owned())
            .collect();
        for name in &sent {
            assert!(
                declared.iter().any(|&(known, _)| known == name.as_str())
                    || UNDECLARED_QUERY.contains(&(id, name.as_str())),
                "{id}: the contract declares no query parameter {name}"
            );
        }
        for &(name, required) in &declared {
            assert!(
                !required || sent.iter().any(|given| given == name),
                "{id}: no required query parameter {name}"
            );
        }

        let token = request.headers.get(TOKEN);
        assert!(
            token.is_some() || !Contract::needs_token(operation),
            "{id}: no token"
        );
        assert!(
            endpoint != Endpoint::SignIn || token.is_none(),
            "{id}: a token"
        );

        match operation.get("requestBody") {
            Some(body) => {
                let schema = &contract.resolve(body)["content"]["application/json"]["schema"];
                let mut shape = Shape::default();
                contract.object(schema, &mut shape);
                assert_eq!(
                    request.headers.get("content-type").map(String::as_str),
                    Some("application/json"),
                    "{id}"
                );
                let fields: Map<String, Value> =
                    serde_json::from_slice(&request.body).expect("a JSON object");
                for field in fields.keys() {
                    assert!(
                        shape.properties.contains(field.as_str()),
                        "{id}: the contract declares no body field {field}"
                    );
                }
                for field in &shape.required {
                    assert!(
                        fields.contains_key(*field),
                        "{id}: no required body field {field}"
                    );
                }
            }
            None if UNDECLARED_EMPTY_BODY.contains(&id) => assert_eq!(request.body, b"{}", "{id}"),
            None => assert!(
                request.body.is_empty(),
                "{id}: a body that the contract does not declare"
            ),
        }
        endpoint
    }

    /// Every call follows the official contract (`contract/openapi.json`):
    /// its method and path under the path of the server, the query
    /// parameters and the body fields that it declares, and the token
    /// where it requires one; the answers that it describes decode; and
    /// the `X-Token` of an answer replaces the token of later calls.
    #[test]
    fn follows_the_contract() {
        let fake = Fake::start("/season/", contract_answer);
        let client = call_every_endpoint(&fake.server());
        let contract = Contract::load();
        let received = fake.received();
        let called: BTreeSet<&str> = received
            .iter()
            .map(|request| operation_id(check_request(&contract, request)))
            .collect();
        assert_eq!(called.len(), ENDPOINTS.len(), "every endpoint is called");

        let tokens: Vec<Option<&str>> = received
            .iter()
            .map(|request| request.headers.get(TOKEN).map(String::as_str))
            .collect();
        assert_eq!(tokens[..3], [None, Some("signed"), Some("rotated")]);
        assert!(tokens[2..].iter().all(|token| *token == Some("rotated")));
        for request in &received[1..] {
            assert_eq!(request.headers.get(USERNAME), request.headers.get(TOKEN));
        }
        assert!(
            !format!("{client:?}").contains("rotated"),
            "the debug output hides the token"
        );
    }

    /// The client follows no redirect, so that the token goes to the
    /// configured server only.
    #[test]
    fn follows_no_redirect() {
        let elsewhere = Fake::start("/", |_| {
            Answer::json(&json!({"ok": 1, "username": "x", "cpu": 1}))
        });
        let location = format!("{}api/auth/me", elsewhere.url);
        let fake = Fake::start("/", move |_| Answer {
            status: 302,
            headers: vec![("location", location.clone())],
            body: String::new(),
        });
        let mut client = token_client(&fake.server(), Access::ReadOnly);
        assert!(matches!(
            client.me(),
            Err(Error::Status { status, .. }) if status == StatusCode::FOUND
        ));
        assert_eq!(fake.received().len(), 1);
        assert!(elsewhere.received().is_empty());
    }

    /// A call that changes the world goes out once: a failure is not
    /// retried, and may have happened.
    #[test]
    fn sends_world_changes_once() {
        let fake = Fake::start("/", |_| Answer {
            status: 502,
            headers: Vec::new(),
            body: "<html>".to_owned(),
        });
        let mut client = token_client(&fake.server(), Access::ReadWrite);
        let branch: BranchName = "main".parse().expect("a branch");
        let error = client
            .set_code(&branch, &Modules::default())
            .expect_err("a bad gateway");
        assert!(matches!(error, Error::Status { .. }) && !error.unapplied());
        assert_eq!(fake.received().len(), 1);
    }

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

    /// A read-only client refuses world changes before it sends them.
    #[test]
    fn read_only() {
        // reqwest takes TLS from the process-wide provider, which `main`
        // installs.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let server = ServerUrl::try_from("http://127.0.0.1:9".to_owned()).expect("a URL");
        let mut client = Client::sign_in(
            &server,
            &Credentials::Token(Sensitive::new("t".to_owned())),
            Access::ReadOnly,
        )
        .expect("a client");
        assert!(matches!(
            client.respawn(),
            Err(Error::ReadOnly {
                endpoint: Endpoint::Respawn
            })
        ));
        let spawn = PlaceSpawn {
            room: "W1N1",
            name: "Spawn1",
            x: 25,
            y: 25,
            shard: Some("shard3"),
        };
        assert!(matches!(
            client.place_spawn(&spawn),
            Err(Error::ReadOnly { .. })
        ));
        assert_eq!(
            serde_json::to_value(&spawn).expect("JSON"),
            serde_json::json!({"room": "W1N1", "name": "Spawn1", "x": 25, "y": 25, "shard": "shard3"})
        );
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
