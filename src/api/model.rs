//! The data that the calls send and decode: only the fields that the
//! manager reads, so that a server that omits or retypes the others still
//! works.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::branch::Active;

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

/// The spawn that [`Client::place_spawn`](super::Client::place_spawn)
/// places.
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

#[cfg(test)]
mod tests {
    use super::*;

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
