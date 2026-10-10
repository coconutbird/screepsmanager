//! The spawn table of a profile: how `poll` keeps a spawn of the account
//! in the world, with each number checked against its bounds.

use std::fmt;
use std::ops::RangeInclusive;
use std::time::Duration;

use serde::Deserialize;

use super::name::{InvalidName, ShardName};

/// The shard of a spawn: `auto`, the one shard with CPU, or a name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(crate) enum ShardChoice {
    /// The only shard where the account has CPU.
    Auto,
    /// The shard of this name.
    Named(ShardName),
}

impl TryFrom<String> for ShardChoice {
    type Error = InvalidName;

    fn try_from(name: String) -> Result<Self, InvalidName> {
        if name == "auto" {
            Ok(Self::Auto)
        } else {
            ShardName::try_from(name).map(Self::Named)
        }
    }
}

impl fmt::Display for ShardChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.pad("auto"),
            Self::Named(name) => name.fmt(f),
        }
    }
}

/// How `poll` places the spawn of a profile's account.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "SpawnFile")]
pub(crate) struct Spawn {
    /// The program and arguments of the selector (no shell), which runs in
    /// the directory of the configuration file.
    pub(crate) selector: Vec<String>,
    /// The shard; none on a private server without shards.
    pub(crate) shard: Option<ShardChoice>,
    /// The time between two polls.
    pub(crate) interval: Duration,
    /// How many rooms around the start room of the server, in each
    /// direction, the candidates come from.
    pub(crate) radius: u32,
    /// The most candidate rooms that the selector chooses from.
    pub(crate) candidates: usize,
}

impl Spawn {
    /// The bounds of `interval`, in seconds: the official server allows
    /// one Memory read a minute.
    pub(crate) const INTERVAL: RangeInclusive<u64> = 60..=86_400;
    /// The bounds of `radius`.
    pub(crate) const RADIUS: RangeInclusive<u64> = 1..=10;
    /// The bounds of `candidates`.
    pub(crate) const CANDIDATES: RangeInclusive<u64> = 1..=64;
    /// The default of `interval`, in seconds.
    const DEFAULT_INTERVAL: u64 = 60;
    /// The default of `radius`.
    const DEFAULT_RADIUS: u64 = 5;
    /// The default of `candidates`.
    const DEFAULT_CANDIDATES: u64 = 16;
}

/// A spawn table as the file writes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnFile {
    selector: Vec<String>,
    shard: Option<ShardChoice>,
    interval: Option<u64>,
    radius: Option<u64>,
    candidates: Option<u64>,
}

impl TryFrom<SpawnFile> for Spawn {
    type Error = SpawnError;

    fn try_from(file: SpawnFile) -> Result<Self, SpawnError> {
        if file.selector.first().is_none_or(String::is_empty) {
            return Err(SpawnError::Selector);
        }
        let bounded = |key, value: Option<u64>, default, range: RangeInclusive<u64>| {
            let value = value.unwrap_or(default);
            if range.contains(&value) {
                Ok(value)
            } else {
                Err(SpawnError::Range { key, value, range })
            }
        };
        let interval = bounded(
            "interval",
            file.interval,
            Self::DEFAULT_INTERVAL,
            Self::INTERVAL,
        )?;
        let radius = bounded("radius", file.radius, Self::DEFAULT_RADIUS, Self::RADIUS)?;
        let candidates = bounded(
            "candidates",
            file.candidates,
            Self::DEFAULT_CANDIDATES,
            Self::CANDIDATES,
        )?;
        Ok(Self {
            selector: file.selector,
            shard: file.shard,
            interval: Duration::from_secs(interval),
            radius: u32::try_from(radius).map_err(|_| SpawnError::Range {
                key: "radius",
                value: radius,
                range: Self::RADIUS,
            })?,
            candidates: usize::try_from(candidates).map_err(|_| SpawnError::Range {
                key: "candidates",
                value: candidates,
                range: Self::CANDIDATES,
            })?,
        })
    }
}

/// A spawn table that is not valid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SpawnError {
    /// The selector has no program.
    #[error("`selector` is the program and its arguments, the program not empty")]
    Selector,
    /// A number is out of its bounds.
    #[error("`{key}` = {value}: use {}..={}", .range.start(), .range.end())]
    Range {
        /// The key.
        key: &'static str,
        /// The value.
        value: u64,
        /// The bounds.
        range: RangeInclusive<u64>,
    },
}
