//! The client of one server: sign-in, the calls that the manager makes,
//! the read-only gate, and the token that answers rotate.

use std::collections::BTreeMap;
use std::fmt;

use reqwest::blocking::Client as Http;
use reqwest::header::HeaderValue;
use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::endpoint::Endpoint;
use super::error::Error;
use super::model::{BranchInfo, Me, PlaceSpawn, WorldStatus};
use super::transport::{self, TOKEN, USERNAME, query};
use crate::branch::{Active, BranchName};
use crate::config::{Credentials, Sensitive, ServerUrl};
use crate::modules::Modules;

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

        let http = transport::http()?;
        let token = match credentials {
            Credentials::Token(token) => token.clone(),
            Credentials::Password { email, password } => {
                let body = SignIn {
                    email,
                    password: password.expose(),
                };
                let request = transport::request(&http, url, Endpoint::SignIn, &[])?.json(&body);
                let reply: SignedIn = transport::call(Endpoint::SignIn, request)?.0;
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
        let mut request = transport::request(&self.http, &self.url, endpoint, query)?
            .header(TOKEN, self.token.clone())
            .header(USERNAME, self.token.clone());
        if let Some(body) = body {
            request = request.json(body);
        }
        let (data, token) = transport::call(endpoint, request)?;
        if let Some(mut token) = token {
            token.set_sensitive(true);
            self.token = token;
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;

    use super::*;
    use crate::api::fake::{Answer, Fake};

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

    /// The client follows no redirect, so that the token goes to the
    /// configured server only.
    #[test]
    fn follows_no_redirect() {
        let elsewhere = Fake::start("/", |_| {
            Answer::json(&serde_json::json!({"ok": 1, "username": "x", "cpu": 1}))
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
}
