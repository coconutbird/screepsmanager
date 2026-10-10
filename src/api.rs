//! The calls of the Screeps API that `upload` and `poll` make
//! ([`Endpoint`](endpoint::Endpoint)). The paths are under the URL of the
//! server (`https://screeps.com/season/`):
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
//! account or the world
//! ([`Endpoint::changes_world`](endpoint::Endpoint::changes_world)) before
//! it sends it.
//!
//! The calls follow the official Screeps World HTTP contract of
//! <https://github.com/openscreeps/openapi>, which `contract/openapi.json`
//! pins (`contract/NOTICE`): the tests send every call to a fake server and
//! check each request against the operation of the contract, and list where
//! the client knowingly differs from it. The client decodes only the fields
//! that the manager reads, so that a private server that omits the others
//! or types them differently (see the standalone profile of the contract)
//! still works.
//!
//! [`client`] makes the calls and holds the token; [`transport`] sends a
//! request and decodes its answer; [`endpoint`] names the calls and which
//! of them change the world; [`model`] has the data that the calls send
//! and decode; [`error`] has the failures.

mod client;
mod endpoint;
mod error;
mod model;
mod transport;

#[cfg(test)]
mod contract;
#[cfg(test)]
mod fake;

pub(crate) use client::{Access, Client};
pub(crate) use error::Error;
pub(crate) use model::{Me, PlaceSpawn, WorldStatus};
