//! The spawn selector: a local program of the bot (`selector` of a spawn
//! table) that chooses the room and the tile of the first spawn from the
//! candidates that `poll` offers. It runs without a shell, in the directory
//! of the configuration, with a time limit and bounded output.
//!
//! Its stdin is one JSON record:
//!
//! ```json
//! {"version": 1, "shard": "shard3", "rooms": [{"name": "W1N1",
//!   "terrain": "2500 digits 0..3", "objects": [...], "status": {...}}]}
//! ```
//!
//! Its stdout is one JSON record, and nothing else (diagnostics go to
//! stderr):
//!
//! ```json
//! {"version": 1, "room": "W1N1", "x": 25, "y": 25, "reason": "..."}
//! {"version": 1, "room": null, "reason": "no suitable room"}
//! ```
//!
//! The answer must name a candidate and a tile 0 to 49; `poll` checks the
//! tile itself before it places a spawn.

use std::io::{self, Read, Write as _};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The version of the protocol.
const VERSION: u64 = 1;
/// The longest run of the selector.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(60);
/// The most bytes of stdout, and of stderr, that the selector may write.
pub(crate) const OUTPUT: usize = 64 * 1024;
/// How long the output may stay open after the selector exits.
const GRACE: Duration = Duration::from_secs(2);
/// How often to check whether the selector exited.
const TICK: Duration = Duration::from_millis(10);

/// A room that the selector may choose.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Candidate {
    /// The name of the room.
    pub(crate) name: String,
    /// The encoded terrain: 2500 digits 0 to 3, row by row.
    pub(crate) terrain: String,
    /// The objects of the room, as the server has them.
    pub(crate) objects: Vec<Value>,
    /// The status fields of the room, as the server has them.
    pub(crate) status: Map<String, Value>,
}

/// The input of the selector.
#[derive(Serialize)]
struct Input<'a> {
    version: u64,
    shard: Option<&'a str>,
    rooms: &'a [Candidate],
}

/// The answer of the selector as it writes it.
#[derive(Deserialize)]
struct Answer {
    version: u64,
    room: Option<String>,
    #[serde(default)]
    x: Option<i64>,
    #[serde(default)]
    y: Option<i64>,
    reason: String,
}

/// What the selector chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Choice {
    /// The spawn at `x`, `y` of the candidate `room`.
    Room {
        /// The room.
        room: String,
        /// The column.
        x: u8,
        /// The row.
        y: u8,
        /// Why.
        reason: String,
    },
    /// No candidate suits.
    Nothing {
        /// Why.
        reason: String,
    },
}

/// What a run of the selector chose, and what it wrote to stderr.
#[derive(Debug)]
pub(crate) struct Selection {
    /// The choice.
    pub(crate) choice: Choice,
    /// The diagnostics of the selector, at most [`OUTPUT`] bytes.
    pub(crate) stderr: String,
}

/// Runs the selector `argv` in `cwd` on the candidates `rooms` of `shard`.
///
/// # Errors
///
/// When the selector does not run, exceeds its limits, fails, or answers
/// against the protocol.
pub(crate) fn choose(
    argv: &[String],
    cwd: &Path,
    shard: Option<&str>,
    rooms: &[Candidate],
) -> Result<Selection, Error> {
    let input = serde_json::to_vec(&Input {
        version: VERSION,
        shard,
        rooms,
    })
    .map_err(Error::Input)?;
    let run = run(argv, cwd, input, TIMEOUT, OUTPUT)?;
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    if !run.status.success() {
        return Err(Error::Failed {
            status: run.status,
            stderr: last_line(&stderr),
        });
    }
    if run.overflow {
        return Err(Error::TooLong);
    }
    Ok(Selection {
        choice: parse(&run.stdout, rooms)?,
        stderr,
    })
}

/// The choice of the stdout `stdout` among `rooms`.
fn parse(stdout: &[u8], rooms: &[Candidate]) -> Result<Choice, Error> {
    let answer: Answer = serde_json::from_slice(stdout)
        .map_err(|error| Error::Protocol(format!("not one JSON answer: {error}")))?;
    if answer.version != VERSION {
        return Err(Error::Protocol(format!(
            "version {}, not {VERSION}",
            answer.version
        )));
    }
    let Some(room) = answer.room else {
        if answer.x.is_some() || answer.y.is_some() {
            return Err(Error::Protocol("x and y without a room".to_owned()));
        }
        return Ok(Choice::Nothing {
            reason: answer.reason,
        });
    };
    if !rooms.iter().any(|candidate| candidate.name == room) {
        return Err(Error::Protocol(format!("room {room} is not a candidate")));
    }
    let tile = |value: Option<i64>, axis| {
        value
            .and_then(|value| u8::try_from(value).ok())
            .filter(|&value| value < 50)
            .ok_or_else(|| Error::Protocol(format!("{axis} is not a tile 0 to 49")))
    };
    Ok(Choice::Room {
        x: tile(answer.x, "x")?,
        y: tile(answer.y, "y")?,
        room,
        reason: answer.reason,
    })
}

/// The last non-empty line of `text`, for an error.
fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("no diagnostics on stderr")
        .trim()
        .to_owned()
}

/// A finished run of a program.
#[derive(Debug)]
struct Run {
    /// How it exited.
    status: ExitStatus,
    /// Its stdout, at most the limit.
    stdout: Vec<u8>,
    /// Whether stdout went beyond the limit.
    overflow: bool,
    /// Its stderr, at most the limit.
    stderr: Vec<u8>,
}

/// An output stream of the program.
enum Stream {
    /// stdout.
    Out,
    /// stderr.
    Err,
}

/// Runs `argv` in `cwd` with `input` on stdin, for at most `timeout`, and
/// keeps at most `limit` bytes of each output.
fn run(
    argv: &[String],
    cwd: &Path,
    input: Vec<u8>,
    timeout: Duration,
    limit: usize,
) -> Result<Run, Error> {
    let (program, arguments) = argv.split_first().ok_or(Error::NoProgram)?;
    let mut child = Command::new(program)
        .args(arguments)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| Error::Spawn {
            program: program.clone(),
            source,
        })?;
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        stop(&mut child);
        return Err(Error::Pipes);
    };
    // The selector may exit without reading everything; its exit status
    // and answer tell, not a broken pipe.
    thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let (sender, receiver) = mpsc::channel();
    let out = sender.clone();
    thread::spawn(move || {
        let _ = out.send((Stream::Out, read_bounded(stdout, limit)));
    });
    thread::spawn(move || {
        let _ = sender.send((Stream::Err, read_bounded(stderr, limit)));
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(TICK),
            Ok(None) => {
                stop(&mut child);
                return Err(Error::Timeout(timeout));
            }
            Err(source) => {
                stop(&mut child);
                return Err(Error::Wait(source));
            }
        }
    };
    let grace = Instant::now() + GRACE;
    let (mut stdout, mut stderr) = (None, None);
    while stdout.is_none() || stderr.is_none() {
        let left = grace.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(left) {
            Ok((Stream::Out, read)) => stdout = Some(read.map_err(Error::Read)?),
            Ok((Stream::Err, read)) => stderr = Some(read.map_err(Error::Read)?),
            Err(_) => return Err(Error::Unclosed),
        }
    }
    let (Some((stdout, overflow)), Some((stderr, _))) = (stdout, stderr) else {
        return Err(Error::Unclosed);
    };
    Ok(Run {
        status,
        stdout,
        overflow,
        stderr,
    })
}

/// Kills `child` and reaps it.
fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// At most `limit` bytes of `reader`, and whether it had more, which it
/// reads to the end without keeping, so that the writer does not block.
fn read_bounded(mut reader: impl Read, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let most = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    (&mut reader).take(most).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        bytes.truncate(limit);
        io::copy(&mut reader, &mut io::sink())?;
        Ok((bytes, true))
    } else {
        Ok((bytes, false))
    }
}

/// A run of the selector that failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The configuration names no program.
    #[error("no program")]
    NoProgram,
    /// The input does not serialize.
    #[error("the input: {0}")]
    Input(#[source] serde_json::Error),
    /// The program does not start.
    #[error("{program}: {source}")]
    Spawn {
        /// The program.
        program: String,
        /// Why.
        source: io::Error,
    },
    /// The pipes of the program are missing.
    #[error("the pipes of the program are not available")]
    Pipes,
    /// Waiting for the program failed.
    #[error("waiting: {0}")]
    Wait(#[source] io::Error),
    /// Reading the output failed.
    #[error("reading the output: {0}")]
    Read(#[source] io::Error),
    /// The program ran too long, and was killed.
    #[error("no answer within {} s; killed", .0.as_secs())]
    Timeout(Duration),
    /// The output stayed open after the program exited (a child of it holds
    /// it).
    #[error("the output stayed open after the program exited")]
    Unclosed,
    /// The program failed.
    #[error("failed ({status}): {stderr}")]
    Failed {
        /// How it exited.
        status: ExitStatus,
        /// The last line of its stderr.
        stderr: String,
    },
    /// The program wrote more than [`OUTPUT`] bytes to stdout.
    #[error("more than {} KiB on stdout", OUTPUT / 1024)]
    TooLong,
    /// The answer is not of the protocol.
    #[error("the answer: {0}")]
    Protocol(String),
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A candidate named `name`.
    fn candidate(name: &str) -> Candidate {
        Candidate {
            name: name.to_owned(),
            terrain: "0".repeat(2500),
            objects: Vec::new(),
            status: Map::new(),
        }
    }

    /// The choice of the answer `answer` among W1N1 and W2N2.
    fn answer(answer: &Value) -> Result<Choice, Error> {
        parse(
            answer.to_string().as_bytes(),
            &[candidate("W1N1"), candidate("W2N2")],
        )
    }

    /// An answer names a candidate and a tile, or no room; anything else is
    /// against the protocol.
    #[test]
    fn answers() {
        assert_eq!(
            answer(&json!({"version": 1, "room": "W2N2", "x": 25, "y": 30, "reason": "best"})).ok(),
            Some(Choice::Room {
                room: "W2N2".to_owned(),
                x: 25,
                y: 30,
                reason: "best".to_owned()
            })
        );
        assert_eq!(
            answer(&json!({"version": 1, "room": null, "reason": "none"})).ok(),
            Some(Choice::Nothing {
                reason: "none".to_owned()
            })
        );
        for bad in [
            json!({"version": 2, "room": null, "reason": "r"}),
            json!({"version": 1, "room": "W3N3", "x": 25, "y": 25, "reason": "r"}),
            json!({"version": 1, "room": "W1N1", "x": 50, "y": 25, "reason": "r"}),
            json!({"version": 1, "room": "W1N1", "x": -1, "y": 25, "reason": "r"}),
            json!({"version": 1, "room": "W1N1", "x": 25.5, "y": 25, "reason": "r"}),
            json!({"version": 1, "room": "W1N1", "x": 25, "reason": "r"}),
            json!({"version": 1, "room": null, "x": 1, "y": 1, "reason": "r"}),
            json!({"version": 1, "room": "W1N1", "x": 25, "y": 25}),
        ] {
            assert!(matches!(answer(&bad), Err(Error::Protocol(_))), "{bad}");
        }
        let two = b"{\"version\":1,\"room\":null,\"reason\":\"a\"}\n{\"version\":1,\"room\":null,\"reason\":\"b\"}";
        assert!(matches!(parse(two, &[]), Err(Error::Protocol(_))));
        assert_eq!(last_line("a\nlast \n\n"), "last");
    }

    /// The bounded reader keeps the limit and drains the rest.
    #[test]
    fn bounded() {
        let (kept, over) = read_bounded(&b"0123456789"[..], 4).expect("reads");
        assert_eq!((kept.as_slice(), over), (&b"0123"[..], true));
        let (kept, over) = read_bounded(&b"0123"[..], 4).expect("reads");
        assert_eq!((kept.as_slice(), over), (&b"0123"[..], false));
    }

    /// A program that copies stdin to stdout.
    fn echo() -> Vec<String> {
        if cfg!(windows) {
            vec!["findstr".to_owned(), "/R".to_owned(), "^".to_owned()]
        } else {
            vec!["cat".to_owned()]
        }
    }

    /// A program that runs for seconds.
    fn slow() -> Vec<String> {
        if cfg!(windows) {
            ["ping", "-n", "6", "127.0.0.1"].map(String::from).to_vec()
        } else {
            ["sleep", "5"].map(String::from).to_vec()
        }
    }

    /// A real program: its stdout comes back within the limit; a slow one
    /// is killed; a missing one does not start.
    #[test]
    fn programs() {
        let text = br#"{"version":1,"room":null,"reason":"none"}"#;
        let echoed = run(&echo(), Path::new("."), text.to_vec(), TIMEOUT, OUTPUT).expect("runs");
        assert!(echoed.status.success());
        assert!(!echoed.overflow);
        assert_eq!(
            parse(&echoed.stdout, &[]).ok(),
            Some(Choice::Nothing {
                reason: "none".to_owned()
            })
        );

        let small = super::run(&echo(), Path::new("."), text.to_vec(), TIMEOUT, 8).expect("runs");
        assert!(small.overflow);
        assert_eq!(small.stdout.len(), 8);

        let started = Instant::now();
        let slow = super::run(
            &slow(),
            Path::new("."),
            Vec::new(),
            Duration::from_millis(300),
            OUTPUT,
        );
        assert!(matches!(slow, Err(Error::Timeout(_))), "{slow:?}");
        assert!(started.elapsed() < Duration::from_secs(4));

        let missing = vec!["screepsmanager-no-such-selector".to_owned()];
        assert!(matches!(
            super::run(&missing, Path::new("."), Vec::new(), TIMEOUT, OUTPUT),
            Err(Error::Spawn { .. })
        ));
        assert!(matches!(
            super::run(&[], Path::new("."), Vec::new(), TIMEOUT, OUTPUT),
            Err(Error::NoProgram)
        ));
    }
}
