//! Rooms of the world: their names and positions, their terrain, and
//! whether a room and a tile take the first spawn of an account. The checks
//! are the manager's own: the selector chooses, these decide whether the
//! choice may be placed.

use std::fmt;
use std::str::FromStr;

use serde_json::{Map, Value};

/// The tiles of a room in each direction.
const SIZE: u8 = 50;
/// The largest room number of a name that this manager takes.
const MAX_NUMBER: u32 = 1000;
/// The object types that mark a room of the Source Keepers or the Invaders.
const HOSTILE_TYPES: [&str; 2] = ["keeperLair", "invaderCore"];
/// The object types that keep the owner of what they were, but are no
/// presence in the room.
const REMAINS_TYPES: [&str; 2] = ["tombstone", "ruin"];

/// The name of a room (`W12N3`), as a position in the world: `E0` is
/// column 0 and `W0` column -1; `S0` is row 0 and `N0` row -1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RoomName {
    x: i32,
    y: i32,
}

impl RoomName {
    /// The number of the column in the name (`12` of `W12N3`).
    fn column(self) -> u32 {
        number(self.x)
    }

    /// The number of the row in the name (`3` of `W12N3`).
    fn row(self) -> u32 {
        number(self.y)
    }

    /// Whether the room can have a controller in the standard world: not a
    /// highway (a number that ends in 0) and not the nine rooms at the
    /// center of a sector (both numbers end in 4 to 6: Source Keepers and
    /// the center room).
    pub(crate) fn may_have_controller(self) -> bool {
        let (column, row) = (self.column() % 10, self.row() % 10);
        let center = |n| (4..=6).contains(&n);
        column != 0 && row != 0 && !(center(column) && center(row))
    }

    /// The distance to `other` in rooms, diagonals counting one.
    pub(crate) fn distance(self, other: Self) -> u32 {
        self.x.abs_diff(other.x).max(self.y.abs_diff(other.y))
    }

    /// The rooms at most `radius` rooms away, nearest first, then by name.
    pub(crate) fn around(self, radius: u32) -> Vec<Self> {
        let radius = i32::try_from(radius.min(MAX_NUMBER)).unwrap_or(0);
        let mut rooms = Vec::new();
        for y in self.y - radius..=self.y + radius {
            for x in self.x - radius..=self.x + radius {
                let room = Self { x, y };
                if room.column() <= MAX_NUMBER && room.row() <= MAX_NUMBER {
                    rooms.push(room);
                }
            }
        }
        // Each name is formatted once, not twice per comparison.
        rooms.sort_by_cached_key(|room| (self.distance(*room), room.to_string()));
        rooms
    }
}

/// The number in a name of the world position `position`.
fn number(position: i32) -> u32 {
    if position < 0 {
        position.unsigned_abs() - 1
    } else {
        position.unsigned_abs()
    }
}

impl FromStr for RoomName {
    type Err = InvalidRoom;

    fn from_str(name: &str) -> Result<Self, InvalidRoom> {
        let invalid = || InvalidRoom(name.to_owned());
        let split = name.find(['N', 'S']).ok_or_else(invalid)?;
        let (horizontal, vertical) = name.split_at(split);
        let axis = |part: &str, negative: char, positive: char| {
            let mut chars = part.chars();
            let direction = chars.next().ok_or_else(invalid)?;
            let digits = chars.as_str();
            if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(invalid());
            }
            let value: u32 = digits.parse().map_err(|_| invalid())?;
            if value > MAX_NUMBER {
                return Err(invalid());
            }
            let value = i32::try_from(value).map_err(|_| invalid())?;
            match direction {
                d if d == negative => Ok(-value - 1),
                d if d == positive => Ok(value),
                _ => Err(invalid()),
            }
        };
        Ok(Self {
            x: axis(horizontal, 'W', 'E')?,
            y: axis(vertical, 'N', 'S')?,
        })
    }
}

impl fmt::Display for RoomName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let horizontal = if self.x < 0 { 'W' } else { 'E' };
        let vertical = if self.y < 0 { 'N' } else { 'S' };
        write!(f, "{horizontal}{}{vertical}{}", self.column(), self.row())
    }
}

/// A text that is not a room name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a room name (W12N3)")]
pub(crate) struct InvalidRoom(String);

/// The room part of `name` for `shard`: `W1N1`, or `shard3/W1N1` when the
/// prefix is `shard` (or there is no shard). None for another shard.
pub(crate) fn on_shard<'a>(name: &'a str, shard: Option<&str>) -> Option<&'a str> {
    match name.split_once('/') {
        None => Some(name),
        Some((prefix, room)) if shard.is_none_or(|shard| shard == prefix) => Some(room),
        Some(_) => None,
    }
}

/// The terrain of a room: per tile, row by row, a mask of wall (1) and
/// swamp (2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Terrain {
    encoded: String,
}

impl Terrain {
    /// The terrain of the encoded text `encoded`: 2500 digits 0 to 3.
    ///
    /// # Errors
    ///
    /// When the text has another length or another character.
    pub(crate) fn parse(encoded: String) -> Result<Self, InvalidTerrain> {
        let tiles = usize::from(SIZE) * usize::from(SIZE);
        if encoded.len() != tiles {
            return Err(InvalidTerrain::Length(encoded.len()));
        }
        if let Some(bad) = encoded.chars().find(|c| !matches!(c, '0'..='3')) {
            return Err(InvalidTerrain::Character(bad));
        }
        Ok(Self { encoded })
    }

    /// The encoded text.
    pub(crate) fn as_str(&self) -> &str {
        &self.encoded
    }

    /// Whether the tile at `x`, `y` (each below 50) is a wall.
    fn wall(&self, x: u8, y: u8) -> bool {
        let at = usize::from(y) * usize::from(SIZE) + usize::from(x);
        self.encoded.as_bytes()[at] & 1 != 0
    }
}

/// An encoded terrain that is not valid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum InvalidTerrain {
    /// Not 2500 tiles.
    #[error("{0} tiles, not 2500")]
    Length(usize),
    /// A tile that is not 0 to 3.
    #[error("a tile {0:?}, not 0 to 3")]
    Character(char),
}

/// Why a room or a tile does not take the first spawn.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Unfit {
    /// The room is not open to play.
    #[error("status {0}")]
    Status(String),
    /// The room opens later.
    #[error("not open yet")]
    NotOpen,
    /// The room is on the server's list of rooms prohibited for respawn.
    #[error("prohibited for respawn")]
    Prohibited,
    /// Someone owns or reserves the room.
    #[error("owned or reserved")]
    Owned,
    /// The room has no controller, or several.
    #[error("{0} controllers")]
    Controllers(usize),
    /// The room has Source Keepers or an Invader core.
    #[error("has a {0}")]
    Hostile(String),
    /// The room has an object of a player or of NPCs.
    #[error("has a {0} of another owner")]
    Foreign(String),
    /// A field of the server's answer does not have the type it should.
    #[error("unexpected field {0}")]
    Malformed(&'static str),
    /// The tile is on the edge of the room, or not 0 to 49.
    #[error("on or beyond the edge")]
    Edge,
    /// The tile is a wall.
    #[error("a wall")]
    Wall,
    /// The tile is next to an exit.
    #[error("next to an exit")]
    Exit,
    /// An object is on the tile.
    #[error("a {0} is on the tile")]
    Occupied(String),
}

/// Whether the status fields `status` (of `room-status`, or a `map-stats`
/// entry) let a spawn in: `normal`, open by `now` (milliseconds since the
/// Unix epoch), and without an owner or a reservation (`own`).
///
/// # Errors
///
/// What does not.
pub(crate) fn check_status(status: &Map<String, Value>, now: f64) -> Result<(), Unfit> {
    match status.get("status") {
        Some(Value::String(text)) if text == "normal" => {}
        Some(Value::String(text)) => return Err(Unfit::Status(text.clone())),
        _ => return Err(Unfit::Malformed("status")),
    }
    let open = match status.get("openTime") {
        None | Some(Value::Null) => None,
        Some(Value::Number(time)) => time.as_f64(),
        Some(Value::String(time)) => Some(
            time.parse::<f64>()
                .map_err(|_| Unfit::Malformed("openTime"))?,
        ),
        Some(_) => return Err(Unfit::Malformed("openTime")),
    };
    if open.is_some_and(|open| open > now) {
        return Err(Unfit::NotOpen);
    }
    match status.get("own") {
        None | Some(Value::Null) => Ok(()),
        Some(_) => Err(Unfit::Owned),
    }
}

/// Whether the objects `objects` of a room let a spawn in: one neutral,
/// unreserved controller, and nothing of another owner (but remains).
///
/// # Errors
///
/// What does not.
pub(crate) fn check_objects(objects: &[Value]) -> Result<(), Unfit> {
    let mut controllers = 0;
    for object in objects {
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or(Unfit::Malformed("type"))?;
        if HOSTILE_TYPES.contains(&kind) {
            return Err(Unfit::Hostile(kind.to_owned()));
        }
        let owned = |field| object.get(field).is_some_and(|value| !value.is_null());
        if kind == "controller" {
            controllers += 1;
            if owned("user") || owned("reservation") {
                return Err(Unfit::Owned);
            }
        } else if owned("user") && !REMAINS_TYPES.contains(&kind) {
            return Err(Unfit::Foreign(kind.to_owned()));
        }
    }
    if controllers == 1 {
        Ok(())
    } else {
        Err(Unfit::Controllers(controllers))
    }
}

/// Whether a spawn may stand at `x`, `y` of a room of terrain `terrain`
/// and objects `objects`: inside the edge, not a wall, not next to an exit
/// (the engine's rule for structures on tiles 1 and 48), and free of
/// objects.
///
/// # Errors
///
/// What does not.
pub(crate) fn check_tile(terrain: &Terrain, objects: &[Value], x: u8, y: u8) -> Result<(), Unfit> {
    let last = SIZE - 1;
    if x == 0 || y == 0 || x >= last || y >= last {
        return Err(Unfit::Edge);
    }
    if terrain.wall(x, y) {
        return Err(Unfit::Wall);
    }
    for ny in y - 1..=y + 1 {
        for nx in x - 1..=x + 1 {
            let edge = nx == 0 || ny == 0 || nx == last || ny == last;
            if edge && !terrain.wall(nx, ny) {
                return Err(Unfit::Exit);
            }
        }
    }
    for object in objects {
        let at = |field| {
            object
                .get(field)
                .and_then(Value::as_u64)
                .ok_or(Unfit::Malformed("x/y"))
        };
        if (at("x")?, at("y")?) == (u64::from(x), u64::from(y)) {
            let kind = object
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("object");
            return Err(Unfit::Occupied(kind.to_owned()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A room name.
    fn room(name: &str) -> RoomName {
        name.parse().expect("a room name")
    }

    /// Names round-trip; W0 and E0 are neighbors.
    #[test]
    fn names() {
        for name in ["W0N0", "E0S0", "W12N3", "E1000S999"] {
            assert_eq!(room(name).to_string(), name);
        }
        assert_eq!(room("W0N0").distance(room("E0N0")), 1);
        assert_eq!(room("W0N0").distance(room("E1S1")), 2);
        for invalid in [
            "", "W1", "X1N1", "W-1N1", "WN1", "W1N1x", "w1n1", "W1001N1", "W1N1/",
        ] {
            assert!(invalid.parse::<RoomName>().is_err(), "{invalid}");
        }
        assert_eq!(on_shard("shard3/W1N1", Some("shard3")), Some("W1N1"));
        assert_eq!(on_shard("shard0/W1N1", Some("shard3")), None);
        assert_eq!(on_shard("W1N1", Some("shard3")), Some("W1N1"));
        assert_eq!(on_shard("shard0/W1N1", None), Some("W1N1"));
    }

    /// Highways and sector centers have no controller.
    #[test]
    fn controllers() {
        for name in ["W1N1", "W3N7", "E9S9", "W14N13", "E4S3"] {
            assert!(room(name).may_have_controller(), "{name}");
        }
        for name in ["W0N1", "E10S5", "W15S5", "W14N16", "E5S5", "W4N6"] {
            assert!(!room(name).may_have_controller(), "{name}");
        }
    }

    /// The rooms around come nearest first, then by name.
    #[test]
    fn around() {
        let rooms: Vec<String> = room("W1N1")
            .around(1)
            .into_iter()
            .map(|room| room.to_string())
            .collect();
        assert_eq!(
            rooms,
            [
                "W1N1", "W0N0", "W0N1", "W0N2", "W1N0", "W1N2", "W2N0", "W2N1", "W2N2"
            ]
        );
        assert_eq!(room("E5S5").around(5).len(), 121);
    }

    /// A plain room with walls around the edge but one exit at the top.
    fn terrain() -> Terrain {
        let mut tiles = vec![b'0'; 2500];
        for at in 0..50 {
            for (x, y) in [(at, 0), (at, 49), (0, at), (49, at)] {
                tiles[y * 50 + x] = b'1';
            }
        }
        tiles[10] = b'0'; // an exit at (10, 0)
        tiles[25 * 50 + 30] = b'3'; // a wall at (30, 25)
        tiles[25 * 50 + 31] = b'2'; // a swamp at (31, 25)
        Terrain::parse(String::from_utf8(tiles).expect("ASCII")).expect("valid terrain")
    }

    /// Tiles: the edge, walls, exits, and objects block; swamps do not.
    #[test]
    fn tiles() {
        let terrain = terrain();
        let objects = [json!({"type": "source", "x": 20, "y": 20})];
        assert_eq!(check_tile(&terrain, &objects, 25, 25), Ok(()));
        assert_eq!(check_tile(&terrain, &objects, 31, 25), Ok(()));
        assert_eq!(check_tile(&terrain, &objects, 0, 25), Err(Unfit::Edge));
        assert_eq!(check_tile(&terrain, &objects, 25, 49), Err(Unfit::Edge));
        assert_eq!(check_tile(&terrain, &objects, 30, 25), Err(Unfit::Wall));
        assert_eq!(check_tile(&terrain, &objects, 11, 1), Err(Unfit::Exit));
        assert_eq!(check_tile(&terrain, &objects, 20, 1), Ok(()));
        assert_eq!(
            check_tile(&terrain, &objects, 20, 20),
            Err(Unfit::Occupied("source".to_owned()))
        );
        assert_eq!(
            check_tile(&terrain, &[json!({"type": "source"})], 25, 25),
            Err(Unfit::Malformed("x/y"))
        );
        assert!(Terrain::parse("0".repeat(2499)).is_err());
        assert!(Terrain::parse(format!("{}4", "0".repeat(2499))).is_err());
    }

    /// Rooms: one neutral controller, no keepers, no other owner.
    #[test]
    fn rooms() {
        let controller = json!({"type": "controller", "x": 10, "y": 10, "level": 0});
        let source = json!({"type": "source", "x": 5, "y": 5});
        assert_eq!(check_objects(&[controller.clone(), source.clone()]), Ok(()));
        assert_eq!(
            check_objects(&[
                controller.clone(),
                json!({"type": "tombstone", "user": "u", "x": 1, "y": 1})
            ]),
            Ok(())
        );
        assert_eq!(
            check_objects(std::slice::from_ref(&source)),
            Err(Unfit::Controllers(0))
        );
        let reserved = json!({"type": "controller", "reservation": {"user": "u"}});
        assert_eq!(check_objects(&[reserved]), Err(Unfit::Owned));
        let owned = json!({"type": "controller", "user": "u", "level": 3});
        assert_eq!(check_objects(&[owned]), Err(Unfit::Owned));
        assert_eq!(
            check_objects(&[controller.clone(), json!({"type": "keeperLair"})]),
            Err(Unfit::Hostile("keeperLair".to_owned()))
        );
        assert_eq!(
            check_objects(&[controller, json!({"type": "creep", "user": "2"})]),
            Err(Unfit::Foreign("creep".to_owned()))
        );
    }

    /// Statuses: normal, open, and without an owner.
    #[test]
    fn statuses() {
        let status = |value: Value| check_status(value.as_object().expect("an object"), 1000.0);
        assert_eq!(status(json!({"status": "normal", "novice": null})), Ok(()));
        assert_eq!(status(json!({"status": "normal", "openTime": 999})), Ok(()));
        assert_eq!(
            status(json!({"status": "normal", "openTime": "2000"})),
            Err(Unfit::NotOpen)
        );
        assert_eq!(
            status(json!({"status": "out of borders"})),
            Err(Unfit::Status("out of borders".to_owned()))
        );
        assert_eq!(
            status(json!({"status": "normal", "own": {"user": "u", "level": 0}})),
            Err(Unfit::Owned)
        );
        assert_eq!(status(json!({})), Err(Unfit::Malformed("status")));
    }
}
