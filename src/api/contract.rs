//! The contract test of the client: every call goes to a fake server, and
//! each request is checked against its operation in the pinned official
//! contract (`contract/openapi.json`), with the known differences listed.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use super::endpoint::Endpoint;
use super::fake::{Answer, Fake, Received};
use super::transport::{TOKEN, USERNAME};
use super::{Access, Client, PlaceSpawn, WorldStatus};
use crate::branch::{Active, BranchName};
use crate::config::{Credentials, Sensitive, ServerUrl};
use crate::modules::Modules;

/// The official contract that the client follows: `openapi.json` of
/// <https://github.com/openscreeps/openapi> at commit
/// `1cf3725ab8df0b61028ae3177750217430abb818`, byte for byte (blob
/// `784aa003218a00d62b14f6bf46b8d375f6aabacb`; ISC License, see
/// `contract/NOTICE`).
const CONTRACT: &str = include_str!("../../contract/openapi.json");

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
    let mut client = Client::sign_in(server, &credentials, Access::ReadWrite).expect("signed in");
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
        .find(|endpoint| endpoint.path() == path && endpoint.method().as_str() == request.method)
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
