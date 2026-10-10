//! The calls of the API: their methods, their paths under the URL of the
//! server, and which of them change the world.

use std::fmt;

use reqwest::Method;

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
    pub(super) fn method(self) -> Method {
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
    pub(super) fn path(self) -> &'static str {
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
