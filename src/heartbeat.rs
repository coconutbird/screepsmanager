//! The heartbeat that the bot writes to `Memory.__screepsmanager` every
//! tick: the footholds that it still has on its shard. A reset of a `lost`
//! account needs a fresh heartbeat of every lost shard that counts none;
//! anything else (no heartbeat, an old one, one that does not parse) keeps
//! the account as it is.
//!
//! ```json
//! {"version": 1, "tick": 123, "shard": "shard3", "rooms": 0, "creeps": 0,
//!  "spawns": 0, "sites": 0, "powerCreeps": 0}
//! ```

use std::io::Read as _;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use flate2::read::GzDecoder;
use serde::Deserialize;

/// The Memory path of the heartbeat.
pub(crate) const PATH: &str = "__screepsmanager";
/// The version of the heartbeat that this manager reads.
const VERSION: u64 = 1;
/// The most ticks that a heartbeat may be behind the game time.
pub(crate) const MAX_AGE: u64 = 3;
/// The prefix of gzip data in a Memory answer.
const GZIP: &str = "gz:";
/// The most bytes of a heartbeat, unpacked.
const MAX_TEXT: u64 = 64 * 1024;

/// A heartbeat of the bot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Heartbeat {
    /// The version of the format.
    pub(crate) version: u64,
    /// The game time when the bot wrote it.
    pub(crate) tick: u64,
    /// The shard of the bot.
    pub(crate) shard: String,
    /// The rooms whose controller the bot owns.
    pub(crate) rooms: u64,
    /// The creeps of the bot.
    pub(crate) creeps: u64,
    /// The spawns of the bot.
    pub(crate) spawns: u64,
    /// The construction sites of the bot.
    pub(crate) sites: u64,
    /// The power creeps of the bot in the world.
    pub(crate) power_creeps: u64,
}

impl Heartbeat {
    /// The heartbeat of the Memory answer `data` (`gz:` and base64 of gzip
    /// of JSON, or JSON).
    ///
    /// # Errors
    ///
    /// When there is no heartbeat or it does not parse.
    pub(crate) fn decode(data: Option<&str>) -> Result<Self, Unsafe> {
        let data = data.ok_or(Unsafe::Missing)?;
        let text = match data.strip_prefix(GZIP) {
            Some(packed) => {
                let packed = BASE64
                    .decode(packed.trim())
                    .map_err(|error| Unsafe::Malformed(format!("base64: {error}")))?;
                let mut text = String::new();
                GzDecoder::new(packed.as_slice())
                    .take(MAX_TEXT + 1)
                    .read_to_string(&mut text)
                    .map_err(|error| Unsafe::Malformed(format!("gzip: {error}")))?;
                if !matches!(u64::try_from(text.len()), Ok(len) if len <= MAX_TEXT) {
                    return Err(Unsafe::Malformed("larger than 64 KiB".to_owned()));
                }
                text
            }
            None => data.to_owned(),
        };
        match text.trim() {
            "" | "undefined" | "null" => Err(Unsafe::Missing),
            text => {
                serde_json::from_str(text).map_err(|error| Unsafe::Malformed(error.to_string()))
            }
        }
    }

    /// Whether the heartbeat says that the bot has no foothold left on
    /// `shard` (any shard without one) at the game time `time`.
    ///
    /// # Errors
    ///
    /// When it has another version or shard, is not fresh, or counts a
    /// foothold.
    pub(crate) fn check(&self, shard: Option<&str>, time: u64) -> Result<(), Unsafe> {
        if self.version != VERSION {
            return Err(Unsafe::Version(self.version));
        }
        if let Some(shard) = shard
            && shard != self.shard
        {
            return Err(Unsafe::Shard(self.shard.clone()));
        }
        let Some(age) = time.checked_sub(self.tick) else {
            return Err(Unsafe::Future {
                tick: self.tick,
                time,
            });
        };
        if age > MAX_AGE {
            return Err(Unsafe::Stale(age));
        }
        let footholds = [
            (self.rooms, "rooms"),
            (self.creeps, "creeps"),
            (self.spawns, "spawns"),
            (self.sites, "construction sites"),
            (self.power_creeps, "power creeps"),
        ];
        let held: Vec<String> = footholds
            .iter()
            .filter(|(count, _)| *count > 0)
            .map(|(count, what)| format!("{count} {what}"))
            .collect();
        if held.is_empty() {
            Ok(())
        } else {
            Err(Unsafe::Footholds(held.join(", ")))
        }
    }
}

/// Why a heartbeat does not allow a reset.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Unsafe {
    /// Memory has no heartbeat.
    #[error("no heartbeat in Memory.{PATH}")]
    Missing,
    /// The heartbeat does not parse.
    #[error("the heartbeat does not parse: {0}")]
    Malformed(String),
    /// The heartbeat has another version.
    #[error("heartbeat version {0}, not {VERSION}")]
    Version(u64),
    /// The heartbeat is of another shard.
    #[error("the heartbeat is of shard {0}")]
    Shard(String),
    /// The heartbeat is ahead of the game time.
    #[error("the heartbeat of tick {tick} is ahead of the game time {time}")]
    Future {
        /// The tick of the heartbeat.
        tick: u64,
        /// The game time.
        time: u64,
    },
    /// The heartbeat is old: the bot does not run.
    #[error("the heartbeat is {0} ticks old (at most {MAX_AGE}): the bot does not run")]
    Stale(u64),
    /// The bot still has footholds.
    #[error("the bot still has {0}")]
    Footholds(String),
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::*;

    /// `text` as the API packs Memory.
    fn packed(text: &str) -> String {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(text.as_bytes()).expect("gzip writes");
        format!(
            "gz:{}",
            BASE64.encode(encoder.finish().expect("gzip finishes"))
        )
    }

    /// A heartbeat without footholds at `tick` on shard3.
    const EMPTY: &str = r#"{"version":1,"tick":100,"shard":"shard3","rooms":0,"creeps":0,"spawns":0,"sites":0,"powerCreeps":0}"#;

    /// Packed and plain heartbeats decode; missing and broken ones are
    /// errors.
    #[test]
    fn decodes() {
        let heartbeat = Heartbeat::decode(Some(&packed(EMPTY))).expect("a heartbeat");
        assert_eq!(heartbeat.tick, 100);
        assert_eq!(Heartbeat::decode(Some(EMPTY)), Ok(heartbeat));
        assert_eq!(Heartbeat::decode(None), Err(Unsafe::Missing));
        assert_eq!(
            Heartbeat::decode(Some(&packed("undefined"))),
            Err(Unsafe::Missing)
        );
        for broken in [
            "gz:!!!".to_owned(),
            "gz:aGVsbG8=".to_owned(),
            packed(r#"{"version":1}"#),
            packed(&EMPTY.replace("\"creeps\":0", "\"creeps\":-1")),
            packed(&EMPTY.replace("\"rooms\":0", "\"rooms\":0.5")),
        ] {
            assert!(
                matches!(Heartbeat::decode(Some(&broken)), Err(Unsafe::Malformed(_))),
                "{broken}"
            );
        }
    }

    /// A reset needs version 1, the shard, at most 3 ticks of age, and no
    /// foothold.
    #[test]
    fn checks() {
        let heartbeat = Heartbeat::decode(Some(EMPTY)).expect("a heartbeat");
        assert_eq!(heartbeat.check(Some("shard3"), 100), Ok(()));
        assert_eq!(heartbeat.check(Some("shard3"), 103), Ok(()));
        assert_eq!(heartbeat.check(None, 101), Ok(()));
        assert_eq!(heartbeat.check(Some("shard3"), 104), Err(Unsafe::Stale(4)));
        assert!(matches!(
            heartbeat.check(Some("shard3"), 99),
            Err(Unsafe::Future { .. })
        ));
        assert_eq!(
            heartbeat.check(Some("shard0"), 100),
            Err(Unsafe::Shard("shard3".to_owned()))
        );
        let alive = Heartbeat {
            creeps: 2,
            sites: 1,
            ..heartbeat.clone()
        };
        assert_eq!(
            alive.check(Some("shard3"), 100),
            Err(Unsafe::Footholds(
                "2 creeps, 1 construction sites".to_owned()
            ))
        );
        let future = Heartbeat {
            version: 2,
            ..heartbeat
        };
        assert_eq!(future.check(Some("shard3"), 100), Err(Unsafe::Version(2)));
    }
}
