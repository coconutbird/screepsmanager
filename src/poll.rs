//! The `poll` command: keeps a spawn of the account of each profile with a
//! spawn table in the world. Each poll reads the account, its shards, and
//! its world status:
//!
//! - `normal` (a spawn): nothing to do.
//! - `empty` (no object): the selector of the bot chooses a room and a tile
//!   from candidates near the start room of the server; `poll` checks the
//!   choice itself, reads the room, the account, and the prohibited rooms
//!   again, and places the spawn.
//! - `lost` (objects, no spawn): only when the heartbeat of the bot on every
//!   lost shard is fresh and counts no foothold, for [`CONFIRM`] in a row,
//!   and again right before, does `poll` reset the account; the next polls
//!   place the spawn after the cooldown of the server.
//!
//! Without `--execute`, nothing changes the world: the client is read-only
//! and `poll` prints what it would do. A failed call never counts as an
//! empty or dead account, and restarts the confirmation. No call that
//! changes the world repeats on its own: the next poll reads the world
//! first. `poll` changes no CPU, code, or branch.
//!
//! The official server's world status is of the whole account: every shard
//! answers the same. A lost account therefore needs a fresh heartbeat on
//! every shard, and a shard without CPU runs no code and writes none, so an
//! account with CPU on one shard of several is never reset automatically.
//! That is the price of the proof: a reset never rests on a guess.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::Error;
use crate::api::{self, Access, Client, Me, PlaceSpawn, WorldStatus};
use crate::branch::{Active, BranchName, GitError};
use crate::config::{Config, ProfileName, SecretError, Selected, ServerName, ShardChoice, Spawn};
use crate::envfile::EnvFile;
use crate::heartbeat::{self, Heartbeat};
use crate::room::{self, InvalidTerrain, RoomName, Terrain, Unfit};
use crate::selector::{self, Candidate, Choice};

/// How long a `lost` account without footholds must stay so before a reset.
const CONFIRM: Duration = Duration::from_secs(180);
/// How long after a respawn the server refuses a spawn.
const RESPAWN_COOLDOWN: Duration = Duration::from_secs(180);
/// A margin on the cooldown, for the clocks of the server and this host.
const COOLDOWN_MARGIN: Duration = Duration::from_secs(10);
/// How long a plan that placed no spawn (read-only, no candidate, refused,
/// blocked) stands before the next poll reads the candidates again: the
/// server limits `map-stats` to one call a minute.
const REPLAN: Duration = Duration::from_secs(600);
/// The name of the first spawn.
const SPAWN_NAME: &str = "Spawn1";

/// The arguments of `poll`.
#[derive(Debug, clap::Args)]
pub(crate) struct Poll {
    /// A profile to poll (repeatable, or comma-separated) [default: every
    /// profile with a spawn table]
    #[arg(long = "profile", short, value_name = "NAME", value_delimiter = ',')]
    profiles: Vec<ProfileName>,
    /// Poll once and exit; a reset needs polls over minutes, so once never
    /// resets
    #[arg(long)]
    once: bool,
    /// Reset the account and place the spawn when every check passes;
    /// without it, poll only reads and prints what it would do
    #[arg(long)]
    execute: bool,
}

/// What every poll shares.
struct Context<'a> {
    /// The `--env-file` variables.
    env: &'a EnvFile,
    /// Where the selector runs: the directory of the configuration.
    root: &'a Path,
    /// Whether to change the world.
    execute: bool,
    /// Whether this is the only poll.
    once: bool,
}

impl Poll {
    /// Polls the profiles of `config` with the `--env-file` variables `env`,
    /// once or until the process ends, and prints each step to `out`.
    pub(crate) fn run(
        self,
        config: &Config,
        env: &EnvFile,
        out: &mut dyn Write,
    ) -> Result<(), Error> {
        let targets = config.select_spawn(&self.profiles)?;
        let context = Context {
            env,
            root: config.root(),
            execute: self.execute,
            once: self.once,
        };
        writeln!(
            out,
            "poll {}: {}",
            if self.once { "once" } else { "until stopped" },
            if self.execute {
                "execute: resets and spawns when every check passes"
            } else {
                "read-only: nothing changes the world (--execute changes it)"
            }
        )?;
        let start = Instant::now();
        let mut watches: Vec<Watch<'_>> = targets
            .into_iter()
            .filter_map(|target| {
                let spawn = target.profile.spawn.as_ref()?;
                Some(Watch::new(target, spawn, start))
            })
            .collect();
        let mut failed = None;
        loop {
            let now = Instant::now();
            writeln!(out, "at {}", utc(SystemTime::now()))?;
            for watch in watches.iter_mut().filter(|watch| watch.due <= now) {
                watch.due = now + watch.spawn.interval;
                match watch.poll(&context, out) {
                    Ok(()) => {}
                    Err(Failure::Output(error)) => return Err(Error::Output(error)),
                    Err(failure) if self.once && failed.is_none() => {
                        failed = Some(Error::Poll {
                            profile: watch.target.name.clone(),
                            source: failure,
                        });
                    }
                    Err(failure) => writeln!(out, "  error: {failure}")?,
                }
            }
            if self.once {
                return failed.map_or(Ok(()), Err);
            }
            let next = watches.iter().map(|watch| watch.due).min().unwrap_or(now);
            thread::sleep(next.saturating_duration_since(Instant::now()));
        }
    }
}

/// The shards of a poll: every shard of the server (one `None` without
/// shards), and the shard of the spawn or why there is none.
struct Shards {
    all: Vec<Option<String>>,
    target: Result<Option<String>, String>,
}

/// A lost account without footholds, since when.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Confirm {
    since: Instant,
    lost: Vec<Option<String>>,
}

impl Confirm {
    /// How long `slot` has seen the shards `lost` lost without footholds,
    /// with this sight at `now`: another set of shards starts over.
    fn observe(slot: &mut Option<Self>, lost: &[Option<String>], now: Instant) -> Duration {
        match slot {
            Some(confirm) if confirm.lost == lost => now.saturating_duration_since(confirm.since),
            _ => {
                *slot = Some(Self {
                    since: now,
                    lost: lost.to_vec(),
                });
                Duration::ZERO
            }
        }
    }
}

/// The state of one profile between polls.
struct Watch<'a> {
    target: Selected<'a>,
    spawn: &'a Spawn,
    /// The signed-in client, kept between polls.
    client: Option<Client>,
    /// The confirmation of a lost account.
    confirm: Option<Confirm>,
    /// When this process last asked for a respawn whose outcome may be one.
    respawned: Option<SystemTime>,
    /// The rooms (`shard/room`) where the server refused the spawn.
    refused: BTreeSet<String>,
    /// The terrain of rooms (`shard/room`), which does not change.
    terrain: BTreeMap<String, Terrain>,
    /// When a read-only poll last planned a spawn.
    planned: Option<Instant>,
    /// When the next poll is due.
    due: Instant,
}

impl<'a> Watch<'a> {
    /// The state of the profile `target` with the spawn table `spawn`, due
    /// at `due`.
    fn new(target: Selected<'a>, spawn: &'a Spawn, due: Instant) -> Self {
        Self {
            target,
            spawn,
            client: None,
            confirm: None,
            respawned: None,
            refused: BTreeSet::new(),
            terrain: BTreeMap::new(),
            planned: None,
            due,
        }
    }

    /// One poll: signs in when it has no client, and drops the client when
    /// the server rejects the credentials. A failure restarts the
    /// confirmation of a lost account.
    fn poll(&mut self, context: &Context<'_>, out: &mut dyn Write) -> Result<(), Failure> {
        writeln!(
            out,
            "profile {}: {}",
            self.target.name, self.target.server.url
        )?;
        let mut client = match self.client.take() {
            Some(client) => client,
            None => match self.sign_in(context) {
                Ok(client) => client,
                Err(failure) => {
                    self.confirm = None;
                    return Err(failure);
                }
            },
        };
        let result = self.cycle(&mut client, context, out);
        if !matches!(result, Err(Failure::Api(api::Error::Unauthorized { .. }))) {
            self.client = Some(client);
        }
        if result.is_err() {
            self.confirm = None;
        }
        result
    }

    /// A client of the server of the profile: read-only without
    /// `--execute`.
    fn sign_in(&self, context: &Context<'_>) -> Result<Client, Failure> {
        let server = self.target.server;
        let credentials = server
            .auth
            .resolve(context.env)
            .map_err(|source| Failure::Secret {
                server: self.target.profile.server.clone(),
                source,
            })?;
        let access = if context.execute {
            Access::ReadWrite
        } else {
            Access::ReadOnly
        };
        Ok(Client::sign_in(&server.url, &credentials, access)?)
    }

    /// Reads the world and acts on its status.
    fn cycle(
        &mut self,
        client: &mut Client,
        context: &Context<'_>,
        out: &mut dyn Write,
    ) -> Result<(), Failure> {
        let me = client.me()?;
        writeln!(out, "  account {}: cpu {}", me.username, me.cpu)?;
        let shards = self.shards(client, &me)?;
        let mut statuses = Vec::with_capacity(shards.all.len());
        for shard in &shards.all {
            statuses.push((shard.clone(), client.world_status(shard.as_deref())?));
        }
        let text: Vec<String> = statuses
            .iter()
            .map(|(shard, status)| format!("{} {status}", label(shard.as_deref())))
            .collect();
        writeln!(out, "  world: {}", text.join(", "))?;
        if statuses
            .iter()
            .any(|(_, status)| *status == WorldStatus::Normal)
        {
            self.confirm = None;
            self.planned = None;
            writeln!(out, "  the account has a spawn: nothing to do")?;
            return Ok(());
        }
        let lost: Vec<Option<String>> = statuses
            .into_iter()
            .filter(|(_, status)| *status == WorldStatus::Lost)
            .map(|(shard, _)| shard)
            .collect();
        if lost.is_empty() {
            self.confirm = None;
            self.place(client, &me, &shards, context, out)
        } else {
            self.planned = None;
            self.reset(client, &shards, &lost, context, out)
        }
    }

    /// The shards of the server, and the shard of the spawn: the configured
    /// one, or with `auto` the only one where the account has CPU.
    fn shards(&self, client: &mut Client, me: &Me) -> Result<Shards, Failure> {
        let Some(choice) = &self.spawn.shard else {
            return Ok(Shards {
                all: vec![None],
                target: Ok(None),
            });
        };
        let names = client.shards()?;
        if names.is_empty() {
            return Err(Failure::Shards("the server lists no shard".to_owned()));
        }
        let target = match choice {
            ShardChoice::Named(name) => {
                if !names.iter().any(|shard| shard == name.as_str()) {
                    return Err(Failure::Shards(format!(
                        "no shard {name}; shards: {}",
                        names.join(", ")
                    )));
                }
                if cpu(me, Some(name.as_str())) > 0.0 {
                    Ok(Some(name.to_string()))
                } else {
                    Err(format!(
                        "no CPU on {name}; allocate CPU to it (poll does not)"
                    ))
                }
            }
            ShardChoice::Auto => {
                let with_cpu: Vec<&String> = names
                    .iter()
                    .filter(|shard| cpu(me, Some(shard.as_str())) > 0.0)
                    .collect();
                match with_cpu.as_slice() {
                    [only] => Ok(Some((*only).clone())),
                    [] => Err("no shard has CPU; allocate CPU to one (poll does not)".to_owned()),
                    several => {
                        let several: Vec<String> =
                            several.iter().map(ToString::to_string).collect();
                        return Err(Failure::Shards(format!(
                            "shards {} have CPU: choose one with shard = \"NAME\"",
                            several.join(", ")
                        )));
                    }
                }
            }
        };
        Ok(Shards {
            all: names.into_iter().map(Some).collect(),
            target,
        })
    }

    /// The branch of the profile when it runs in the world, or why not.
    fn branch(&self, client: &mut Client) -> Result<Result<BranchName, String>, Failure> {
        let branch = self.target.profile.branch.resolve(&mut None)?;
        let runs = client
            .branches()?
            .iter()
            .any(|info| info.branch == branch.as_str() && info.runs_in(Active::World));
        Ok(if runs {
            Ok(branch)
        } else {
            Err(format!(
                "branch {branch} does not run in the world; upload it with --activate world first (poll does not)"
            ))
        })
    }

    /// The account is lost on the shards `lost`: confirm that the bot has
    /// no foothold left, over time and right before, then reset it.
    fn reset(
        &mut self,
        client: &mut Client,
        shards: &Shards,
        lost: &[Option<String>],
        context: &Context<'_>,
        out: &mut dyn Write,
    ) -> Result<(), Failure> {
        if let Some(blocker) = footholds(client, lost)? {
            self.confirm = None;
            writeln!(out, "  no reset: {blocker}")?;
            return Ok(());
        }
        let held = Confirm::observe(&mut self.confirm, lost, Instant::now());
        let lost_text: Vec<&str> = lost.iter().map(|shard| label(shard.as_deref())).collect();
        if held < CONFIRM {
            writeln!(
                out,
                "  lost without footholds on {}: confirming, {} of {} s",
                lost_text.join(", "),
                held.as_secs(),
                CONFIRM.as_secs()
            )?;
            if context.once {
                writeln!(out, "  --once never resets: only `poll` confirms over time")?;
            }
            return Ok(());
        }
        writeln!(out, "  lost without footholds for {} s", held.as_secs())?;
        let branch = match self.branch(client)? {
            Ok(branch) => branch,
            Err(blocker) => {
                writeln!(out, "  no reset: {blocker}")?;
                return Ok(());
            }
        };
        if !context.execute {
            writeln!(
                out,
                "  would reset the account; branch {branch} runs in the world (--execute resets)"
            )?;
            return Ok(());
        }
        if let Some(blocker) = recheck_lost(client, &shards.all, lost)? {
            self.confirm = None;
            writeln!(out, "  no reset: {blocker}")?;
            return Ok(());
        }
        self.confirm = None;
        writeln!(out, "  resetting the account (user/respawn)")?;
        match client.respawn() {
            Ok(()) => {
                self.respawned = Some(SystemTime::now());
                writeln!(
                    out,
                    "  reset: the spawn follows after the cooldown of the server"
                )?;
                Ok(())
            }
            Err(error) => {
                if !error.unapplied() {
                    self.respawned = Some(SystemTime::now());
                }
                Err(error.into())
            }
        }
    }

    /// The account is empty: choose a room with the selector, check the
    /// choice, check the world again, and place the spawn.
    fn place(
        &mut self,
        client: &mut Client,
        me: &Me,
        shards: &Shards,
        context: &Context<'_>,
        out: &mut dyn Write,
    ) -> Result<(), Failure> {
        let shard = match &shards.target {
            Ok(shard) => shard.clone(),
            Err(blocker) => {
                writeln!(out, "  no spawn: {blocker}")?;
                return Ok(());
            }
        };
        let shard = shard.as_deref();
        if cpu(me, shard) <= 0.0 {
            writeln!(
                out,
                "  no spawn: the account has no CPU (poll does not allocate it)"
            )?;
            return Ok(());
        }
        if let Some(left) = cooldown(me.last_respawn_date, self.respawned, SystemTime::now()) {
            writeln!(out, "  respawn cooldown: {} s left", left.as_secs())?;
            return Ok(());
        }
        if let Some(planned) = self.planned
            && planned.elapsed() < REPLAN
        {
            writeln!(
                out,
                "  planned {} s ago; the next plan in {} s",
                planned.elapsed().as_secs(),
                REPLAN.saturating_sub(planned.elapsed()).as_secs()
            )?;
            return Ok(());
        }
        let branch = self.branch(client)?;
        let candidates = self.discover(client, shard, out)?;
        self.planned = Some(Instant::now());
        if candidates.is_empty() {
            writeln!(out, "  no candidate room: nothing to place")?;
            return Ok(());
        }
        let selection = selector::choose(&self.spawn.selector, context.root, shard, &candidates)?;
        for line in selection
            .stderr
            .lines()
            .filter(|line| !line.trim().is_empty())
        {
            writeln!(out, "  selector: {line}")?;
        }
        let (room, x, y) = match selection.choice {
            Choice::Nothing { reason } => {
                writeln!(out, "  the selector chose no room: {reason}")?;
                return Ok(());
            }
            Choice::Room { room, x, y, reason } => {
                writeln!(out, "  the selector chose {room} at ({x}, {y}): {reason}")?;
                (room, x, y)
            }
        };
        let key = key(shard, &room);
        let (Some(candidate), Some(terrain)) = (
            candidates.iter().find(|candidate| candidate.name == room),
            self.terrain.get(&key),
        ) else {
            return Err(Failure::Unfit {
                room,
                x,
                y,
                reason: Unfit::Malformed("candidate"),
            });
        };
        if let Err(reason) = room::check_tile(terrain, &candidate.objects, x, y) {
            return Err(Failure::Unfit { room, x, y, reason });
        }
        let branch = match branch {
            Ok(branch) => branch,
            Err(blocker) => {
                writeln!(out, "  no spawn: {blocker}")?;
                return Ok(());
            }
        };
        if !context.execute {
            writeln!(
                out,
                "  would place {SPAWN_NAME} in {room} at ({x}, {y}); branch {branch} runs in the world (--execute places it)"
            )?;
            return Ok(());
        }
        let spawn = PlaceSpawn {
            room: &room,
            name: SPAWN_NAME,
            x,
            y,
            shard,
        };
        if let Some(blocker) = self.recheck_place(client, &shards.all, &spawn, terrain)? {
            writeln!(out, "  no spawn: {blocker}")?;
            return Ok(());
        }
        writeln!(
            out,
            "  placing {SPAWN_NAME} in {room} at ({x}, {y}) (game/place-spawn)"
        )?;
        match client.place_spawn(&spawn) {
            Ok(()) => {
                self.planned = None;
                writeln!(out, "  placed")?;
                Ok(())
            }
            Err(error) => {
                if matches!(error, api::Error::Refused { .. }) {
                    self.refused.insert(key);
                }
                Err(error.into())
            }
        }
    }

    /// The candidates of `shard`: rooms around the start room of the
    /// server, nearest first, that may have a controller, are not
    /// prohibited or refused, are normal and without an owner on the map,
    /// and whose objects and status let a spawn in, at most `candidates`.
    fn discover(
        &mut self,
        client: &mut Client,
        shard: Option<&str>,
        out: &mut dyn Write,
    ) -> Result<Vec<Candidate>, Failure> {
        let starting_rooms = client.world_start_room(shard)?;
        let first = starting_rooms.first().cloned().unwrap_or_default();
        let start: RoomName = room::on_shard(&first, shard)
            .and_then(|name| name.parse::<RoomName>().ok())
            .ok_or_else(|| Failure::StartRoom(first.clone()))?;
        let prohibited = prohibited(client, shard)?;
        let names: Vec<String> = start
            .around(self.spawn.radius)
            .into_iter()
            .filter(|room| room.may_have_controller())
            .map(|room| room.to_string())
            .filter(|name| !prohibited.contains(name) && !self.refused.contains(&key(shard, name)))
            .collect();
        let room_map = if names.is_empty() {
            BTreeMap::new()
        } else {
            client.map_stats(&names, shard)?
        };
        let now = now_ms();
        let mut candidates = Vec::new();
        let mut read = 0;
        for name in &names {
            if candidates.len() >= self.spawn.candidates {
                break;
            }
            let Some(entry) = room_map.get(name).and_then(Value::as_object) else {
                continue;
            };
            if room::check_status(entry, now).is_err() {
                continue;
            }
            read += 1;
            let objects = client.room_objects(name, shard)?;
            if room::check_objects(&objects).is_err() {
                continue;
            }
            let Some(status) = client.room_status(name, shard)? else {
                continue;
            };
            if room::check_status(&status, now).is_err() {
                continue;
            }
            let terrain = self.terrain(client, name, shard)?;
            candidates.push(Candidate {
                name: name.clone(),
                terrain: terrain.as_str().to_owned(),
                objects,
                status,
            });
        }
        writeln!(
            out,
            "  candidates: {} of {read} rooms read, of {} within {} of start room {start} on {}",
            candidates.len(),
            names.len(),
            self.spawn.radius,
            label(shard)
        )?;
        Ok(candidates)
    }

    /// The terrain of `room` on `shard`, read once.
    fn terrain(
        &mut self,
        client: &mut Client,
        room: &str,
        shard: Option<&str>,
    ) -> Result<&Terrain, Failure> {
        let terrain: &Terrain = match self.terrain.entry(key(shard, room)) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let encoded =
                    client
                        .room_terrain(room, shard)?
                        .ok_or_else(|| Failure::NoTerrain {
                            room: room.to_owned(),
                        })?;
                entry.insert(Terrain::parse(encoded).map_err(|source| Failure::Terrain {
                    room: room.to_owned(),
                    source,
                })?)
            }
        };
        Ok(terrain)
    }

    /// Why `spawn` may not be placed right now, from fresh reads: the
    /// account's CPU and cooldown, an empty world on every shard of `all`,
    /// the prohibited rooms, and the status and objects of the room of
    /// terrain `terrain`.
    fn recheck_place(
        &self,
        client: &mut Client,
        all: &[Option<String>],
        spawn: &PlaceSpawn<'_>,
        terrain: &Terrain,
    ) -> Result<Option<String>, Failure> {
        let PlaceSpawn {
            room, x, y, shard, ..
        } = *spawn;
        let me = client.me()?;
        if cpu(&me, shard) <= 0.0 {
            return Ok(Some("the account has no CPU".to_owned()));
        }
        if let Some(left) = cooldown(me.last_respawn_date, self.respawned, SystemTime::now()) {
            return Ok(Some(format!("respawn cooldown: {} s left", left.as_secs())));
        }
        for other in all {
            let status = client.world_status(other.as_deref())?;
            if status != WorldStatus::Empty {
                return Ok(Some(format!(
                    "{}: the world status is now {status}",
                    label(other.as_deref())
                )));
            }
        }
        if prohibited(client, shard)?.contains(room) {
            return Ok(Some(format!("{room}: {}", Unfit::Prohibited)));
        }
        let Some(status) = client.room_status(room, shard)? else {
            return Ok(Some(format!("{room}: the server has no status")));
        };
        let objects = client.room_objects(room, shard)?;
        let checked = room::check_status(&status, now_ms())
            .and_then(|()| room::check_objects(&objects))
            .and_then(|()| room::check_tile(terrain, &objects, x, y));
        Ok(checked
            .err()
            .map(|reason| format!("{room} ({x}, {y}): {reason}")))
    }
}

/// Why the bot may still have footholds on the shards `lost`, from its
/// heartbeat in Memory against the game time, read after it.
fn footholds(client: &mut Client, lost: &[Option<String>]) -> Result<Option<String>, Failure> {
    for shard in lost {
        let shard = shard.as_deref();
        let data = client.memory(heartbeat::PATH, shard)?;
        let time = client.time(shard)?;
        let checked = Heartbeat::decode(data.as_deref()).and_then(|beat| beat.check(shard, time));
        if let Err(reason) = checked {
            return Ok(Some(format!("{}: {reason}", label(shard))));
        }
    }
    Ok(None)
}

/// Why the account may not be reset right now, from fresh reads: a shard
/// of `all` with a spawn, other lost shards than `lost`, or footholds.
fn recheck_lost(
    client: &mut Client,
    all: &[Option<String>],
    lost: &[Option<String>],
) -> Result<Option<String>, Failure> {
    let mut now_lost = Vec::new();
    for shard in all {
        match client.world_status(shard.as_deref())? {
            WorldStatus::Normal => {
                return Ok(Some(format!(
                    "{}: the account has a spawn",
                    label(shard.as_deref())
                )));
            }
            WorldStatus::Lost => now_lost.push(shard.clone()),
            WorldStatus::Empty => {}
        }
    }
    if now_lost != lost {
        return Ok(Some("the lost shards changed".to_owned()));
    }
    footholds(client, lost)
}

/// The rooms of `shard` where the account may not place its spawn.
fn prohibited(client: &mut Client, shard: Option<&str>) -> Result<BTreeSet<String>, Failure> {
    Ok(client
        .respawn_prohibited_rooms(shard)?
        .iter()
        .filter_map(|name| room::on_shard(name, shard))
        .map(str::to_owned)
        .collect())
}

/// The CPU of the account on `shard`.
fn cpu(me: &Me, shard: Option<&str>) -> f64 {
    match (shard, &me.cpu_shard) {
        (Some(shard), Some(cpu)) => cpu.get(shard).copied().unwrap_or(0.0),
        _ => me.cpu,
    }
}

/// How long until the server takes a spawn after the respawn of the
/// account at `server` (milliseconds since the Unix epoch) or of this
/// process at `local`, at `now`; none when it takes one.
fn cooldown(server: Option<f64>, local: Option<SystemTime>, now: SystemTime) -> Option<Duration> {
    let wait = RESPAWN_COOLDOWN + COOLDOWN_MARGIN;
    let server = server
        .filter(|ms| ms.is_finite() && *ms > 0.0)
        .and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok())
        .map(|since| UNIX_EPOCH + since + wait);
    let local = local.map(|at| at + wait);
    let until = server.max(local)?;
    until
        .duration_since(now)
        .ok()
        .filter(|left| !left.is_zero())
}

/// The key of `room` on `shard` in the caches.
fn key(shard: Option<&str>, room: &str) -> String {
    format!("{}/{room}", shard.unwrap_or(""))
}

/// The name of `shard` for output.
fn label(shard: Option<&str>) -> &str {
    shard.unwrap_or("the world")
}

/// The time, in milliseconds since the Unix epoch.
fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1000.0)
}

/// `time` in UTC: `2026-10-10T12:00:00Z`.
fn utc(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The year, month, and day of `days` since 1970-01-01 (Howard Hinnant's
/// `civil_from_days`).
fn civil(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// A poll of a profile that failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Failure {
    /// A secret of the server is not available.
    #[error("server {server}: {source}")]
    Secret {
        /// The server.
        server: ServerName,
        /// Why.
        source: SecretError,
    },
    /// A call of the API failed.
    #[error(transparent)]
    Api(#[from] api::Error),
    /// `auto` names no branch.
    #[error("branch auto: {0}")]
    Git(#[from] GitError),
    /// The selector failed.
    #[error("selector: {0}")]
    Selector(#[from] selector::Error),
    /// The shards do not name the shard of the spawn.
    #[error("shards: {0}")]
    Shards(String),
    /// The start room of the server is not a room of the shard.
    #[error("start room {0:?}: not a room of the shard")]
    StartRoom(String),
    /// The server has no terrain of a room.
    #[error("room {room}: the server has no terrain")]
    NoTerrain {
        /// The room.
        room: String,
    },
    /// The terrain of a room is not valid.
    #[error("room {room}: terrain: {source}")]
    Terrain {
        /// The room.
        room: String,
        /// Why.
        source: InvalidTerrain,
    },
    /// The selector chose a tile that does not take a spawn.
    #[error("the selector chose {room} at ({x}, {y}), which does not take a spawn: {reason}")]
    Unfit {
        /// The room.
        room: String,
        /// The column.
        x: u8,
        /// The row.
        y: u8,
        /// Why.
        reason: Unfit,
    },
    /// The output is not writable.
    #[error("the output: {0}")]
    Output(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dates in UTC.
    #[test]
    fn dates() {
        assert_eq!(utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            utc(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            "2023-11-14T22:13:20Z"
        );
        assert_eq!(
            utc(UNIX_EPOCH + Duration::from_hours(264_384)),
            "2000-02-29T00:00:00Z"
        );
    }

    /// The cooldown runs from the later respawn, server or local, with a
    /// margin.
    #[test]
    fn cooldowns() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let ms = |seconds: u32| Some(f64::from(seconds) * 1000.0);
        assert_eq!(cooldown(None, None, now), None);
        assert_eq!(cooldown(ms(1_000_000 - 500), None, now), None);
        assert_eq!(
            cooldown(ms(1_000_000 - 100), None, now),
            Some(Duration::from_secs(90))
        );
        assert_eq!(
            cooldown(
                ms(1_000_000 - 100),
                Some(now - Duration::from_secs(10)),
                now
            ),
            Some(Duration::from_secs(180))
        );
        assert_eq!(cooldown(Some(f64::NAN), None, now), None);
    }

    /// The confirmation counts while the same shards stay lost, and starts
    /// over for others.
    #[test]
    fn confirmation() {
        let start = Instant::now();
        let shard3 = vec![Some("shard3".to_owned())];
        let mut slot = None;
        assert_eq!(Confirm::observe(&mut slot, &shard3, start), Duration::ZERO);
        assert_eq!(
            Confirm::observe(&mut slot, &shard3, start + Duration::from_secs(120)),
            Duration::from_secs(120)
        );
        let both = vec![Some("shard2".to_owned()), Some("shard3".to_owned())];
        assert_eq!(
            Confirm::observe(&mut slot, &both, start + Duration::from_secs(200)),
            Duration::ZERO
        );
        assert_eq!(
            Confirm::observe(&mut slot, &both, start + Duration::from_secs(390)),
            Duration::from_secs(190)
        );
        assert!(Duration::from_secs(190) >= CONFIRM);
    }

    /// The CPU of a shard, or of the account without shards.
    #[test]
    fn cpus() {
        let me = Me {
            username: "me".to_owned(),
            cpu: 60.0,
            cpu_shard: Some(BTreeMap::from([("shard3".to_owned(), 60.0)])),
            last_respawn_date: None,
        };
        assert!(cpu(&me, Some("shard3")) > 0.0);
        assert!(cpu(&me, Some("shard0")) <= 0.0);
        assert!(cpu(&me, None) > 0.0);
        assert_eq!(key(Some("shard3"), "W1N1"), "shard3/W1N1");
        assert_eq!(key(None, "W1N1"), "/W1N1");
    }
}
