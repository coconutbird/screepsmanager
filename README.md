# screepsmanager

Uploads built Screeps code to the branches of Screeps servers: the official
server, seasonal servers, and private servers. A configuration names the
servers and the profiles that upload to them; one command uploads a build to
one profile or to several. It reads a build directory that any tool made
(Rollup, esbuild, `wasm-pack`, ...), so the build needs no plugin and no
Node.js for the upload. `poll` keeps a spawn of the account in the world:
it places the first spawn where the bot's own selector chooses, and resets
an account only on proof from the bot that nothing of it is left.

## Install

With [mise](https://mise.jdx.dev), from the prebuilt binaries of the GitHub
releases (Linux with glibc 2.35 or newer and macOS, each on x86-64 and arm64;
Windows on x86-64):

```sh
mise use -g github:coconutbird/screepsmanager          # the latest release, for every directory
mise use github:coconutbird/screepsmanager@0.1.0       # pinned in this project's mise.toml
```

mise holds back releases younger than its `minimum_release_age` (24 hours by
default), so `latest` reaches a new release a day after it is published. A
pinned version installs at once, and so does every release of a tool listed
in `minimum_release_age_excludes`:

```toml
# ~/.config/mise/config.toml
[settings]
minimum_release_age_excludes = ["github:coconutbird/screepsmanager"]
```

A bot project then pins the tool and its upload next to its build:

```toml
[tools]
"github:coconutbird/screepsmanager" = "0.1.0"

[tasks.deploy]
run = "screepsmanager upload"
```

mise verifies each download against the digest that GitHub reports and the
build-provenance attestation of the release workflow. To build from source
instead (Rust 1.99 or newer):

```sh
mise use 'cargo:coconutbird/screepsmanager@tag:v0.1.0'
```

## Build

```sh
cargo build --release   # target/release/screepsmanager
mise run ci             # fmt, clippy (pedantic, -D warnings), tests
```

The toolchain is pinned in `rust-toolchain.toml`, and in `mise.toml` too:
mise's rust tool sets `RUSTUP_TOOLCHAIN`, which overrides
`rust-toolchain.toml`.

### Release

Bump `version` in `Cargo.toml`, run `cargo check` so that `Cargo.lock`
follows, commit, and push a tag `v<version>`. CI (`.github/workflows/ci.yml`)
checks the tag against `Cargo.toml`, runs the checks, and builds and attests
one archive per target; when the whole run is green, `release.yml` publishes
the release that mise installs from. A tag with a suffix (`v0.2.0-rc1`) is a
pre-release, which mise's `latest` skips.

## Usage

```sh
screepsmanager profiles                          # the profiles of the configuration
screepsmanager upload                            # the default profiles
screepsmanager upload -p main                    # one profile
screepsmanager upload -p main,season             # several, in order
screepsmanager upload -p dev --branch auto       # the current git branch
screepsmanager upload -p main --activate world   # and make the branch run in the world
screepsmanager upload --dry-run                  # check everything but the server
screepsmanager poll --once                       # read the world once and print the plan
screepsmanager poll -p main                      # keep reading, every interval, and print
screepsmanager poll -p main --execute            # and place the spawn, or reset, when proven
screepsmanager --env-file .env upload -p main    # secrets from a dotenv file
```

Every command reads the nearest `screepsmanager.toml` in the working
directory or a parent (`--config FILE` or `$SCREEPSMANAGER_CONFIG` name
another). `upload [DIR]` reads the modules of the build directory (by
default the `dir` of the configuration) once, then for each profile replaces
every module of its branch, creating a branch that the account does not
have, and makes the branch run where the profile and `--activate` say.

Without `--profile` (or `$SCREEPSMANAGER_PROFILE`), `upload` takes the
`default` profiles of the configuration, or its only profile. Profiles of one
server share one sign-in. The first failure stops the upload: its error goes
to stderr as `screepsmanager: ...` with status 1 (2 for a bad command line).

`--env-file FILE` (or `$SCREEPSMANAGER_ENV_FILE`) reads a dotenv file
(`NAME=value`, quotes, `export`, comments, `${VAR}`) for the `{ env = "..." }`
secrets of every command. Only the given file is read, relative to the working
directory; there is no search. A variable of the process environment wins
over the file, and within the file the first definition wins. A missing or
malformed file is an error that names the file and the line, never a value
or the text of a line. The variables are not exported: the spawn selector
does not see them.

The first `screepsmanager upload -p main,season` with the configuration below
prints:

```text
config /home/me/bot/screepsmanager.toml
build /home/me/bot/dist: 3 modules
  main         js                 60 bytes  main.js
  main.js.map  source map         69 bytes  main.js.map
  main_bg      wasm                8 bytes  main_bg.wasm
profile main: branch main on https://screeps.com/, run in world
  created branch main
  uploaded 3 modules
  branch main runs in world
profile season: branch default on https://screeps.com/season/
  uploaded 3 modules
```

## Configuration

```toml
dir = "dist"         # the build directory, relative to this file (default: dist)
default = ["main"]   # the profiles of `upload` without --profile

[servers.official]
url = "https://screeps.com"
token = { env = "SCREEPS_TOKEN" }

[servers.season]
url = "https://screeps.com/season"
token = { env = "SCREEPS_TOKEN" }

[servers.local]
url = "http://127.0.0.1:21025"
email = "me@example.com"
password = { env = "SCREEPS_LOCAL_PASSWORD" }

[profiles.main]
server = "official"
branch = "main"
activate = ["world"]

[profiles.sim]
server = "official"
branch = "sim"
activate = ["sim"]

[profiles.season]
server = "season"

[profiles.dev]
server = "local"
branch = "auto"
activate = ["world"]

[profiles.main.spawn]
selector = ["node", "scripts/select-spawn.mjs"]
shard = "auto"
```

| Key                     | Meaning                                                                             |
| ----------------------- | ----------------------------------------------------------------------------------- |
| `dir`                   | Build directory, relative to the file. Default `dist`                               |
| `default`               | Profiles of `upload` without `--profile`. Default: the only profile                 |
| `servers.NAME.url`      | URL that the API is under: `http` or `https`; `/season` for the seasonal server     |
| `servers.NAME.token`    | API token (official server: account settings, auth tokens)                          |
| `servers.NAME.email`    | Account email, with `password`, for a private server with screepsmod-auth           |
| `servers.NAME.password` | Account password                                                                    |
| `profiles.NAME.server`  | Server of the profile                                                               |
| `profiles.NAME.branch`  | Branch to upload to; `auto` is the current git branch. Default `default`            |
| `profiles.NAME.activate`| Where the upload makes the branch run: `world`, `sim`. Default: nowhere             |
| `profiles.NAME.spawn.selector`  | Program and arguments of the spawn selector (no shell), run next to the file |
| `profiles.NAME.spawn.shard`     | `auto` (the one shard with CPU), a shard name, or unset on a server without shards |
| `profiles.NAME.spawn.interval`  | Seconds between polls, 60 to 86400. Default 60                             |
| `profiles.NAME.spawn.radius`    | Rooms around the start room that candidates come from, 1 to 10. Default 5 |
| `profiles.NAME.spawn.candidates`| Most rooms that the selector chooses from, 1 to 64. Default 16             |

A server signs in with `token`, or with `email` and `password`. A secret is a
string, or `{ env = "VARIABLE" }` to read it from the environment (or the
`--env-file`) when a command needs it, so that the configuration can be
committed. Names have letters, digits, `-`, `_`, and `.`. A key that the
format does not have is an error.

## Spawning

`poll` polls every profile with a spawn table (or those of `--profile`), one
profile per server: a reset is of the whole account. Each poll signs in once
per process, reads the account (`auth/me`), the shards, and the world status
of every shard, and then:

- **normal** (a spawn): nothing.
- **empty** (no object at all): after the server's 180 s respawn cooldown,
  candidates are the rooms within `radius` of the server's start room, nearest
  first, that can have a controller (not highways, not sector centers), are
  not on the prohibited list or refused before, are `normal` and unowned and
  unreserved on the map (`map-stats`), and whose objects show one neutral,
  unreserved controller, no keeper lair or invader core, and nothing of
  another owner. The selector gets them on stdin and answers on stdout; poll
  then checks that the answer names a candidate and a tile inside the edge,
  off walls, not beside an exit, and free of objects. Before it places
  `Spawn1`, it reads again the CPU, the cooldown, the world status of every
  shard, the prohibited list, and the room's status and objects.
- **lost** (objects, but no spawn): see below.

Without `--execute` the client refuses every call that changes the world
before sending it, and poll prints `would place ...` or `would reset ...`.
With it, a refused placement excludes that room for the process; a call whose
outcome is unknown (a timeout, a server error) is never repeated: the next
poll reads the world first. A plan that placed nothing stands for 10 minutes
before the candidates are read again (the server allows one `map-stats` a
minute). poll changes no CPU, code, or branch: the configured branch must
already run in the world, and the account must have CPU on the shard.
API failures are errors, never an empty or dead account; `--once` exits with
status 1 on the first one, the watch prints it and goes on. The client
follows no redirect, so the token goes to the configured server only.

### Selector protocol

stdin, one JSON record:

```json
{"version": 1, "shard": "shard3", "rooms": [{"name": "W1N1", "terrain": "<2500 digits 0..3, row by row>", "objects": [], "status": {}}]}
```

`shard` is `null` without shards; `objects` and `status` are as the server's
`room-objects` and `room-status` have them. stdout, one JSON record and
nothing else (diagnostics go to stderr, which poll prints):

```json
{"version": 1, "room": "W1N1", "x": 25, "y": 25, "reason": "..."}
{"version": 1, "room": null, "reason": "no suitable room"}
```

The selector runs at most 60 s and writes at most 64 KiB to each stream; a
failure, a timeout, or an answer against the protocol is an error.

### Reset of a lost account

`lost` means no spawn, not no creeps: poll resets (`user/respawn`) only on
proof from the bot. The bot writes every tick

```js
Memory.__screepsmanager = { version: 1, tick: Game.time, shard: Game.shard.name,
  rooms: 0, creeps: 0, spawns: 0, sites: 0, powerCreeps: 0 };
```

with its owned controllers, creeps, spawns, construction sites, and power
creeps in the world. A reset needs, on every lost shard, this heartbeat of
version 1 and of that shard, at most 3 ticks behind `game/time`, with every
count 0; continuously for 180 s; with no shard `normal`; and all of it read
again right before the reset. A missing, old, or malformed heartbeat, or a
failed call, blocks the reset and restarts the 180 s. `--once` never resets.
After a reset, the empty account gets its spawn after the cooldown.

**Limitation**: the official server's world status is of the whole account,
so a lost account is lost on every shard, and every shard needs a fresh
heartbeat. A shard without CPU runs no code and writes none: an account with
CPU on one shard of several is never reset automatically. poll prints why;
reset it by hand. The placement of an empty account needs no heartbeat (an
empty account runs no code).

## Modules

A file directly in the build directory is a module when its name ends in

| Suffix    | Module                               | Uploaded as                                  |
| --------- | ------------------------------------ | -------------------------------------------- |
| `.js`     | the name without `.js` (`main`)      | text                                         |
| `.js.map` | the whole name (`main.js.map`)       | `module.exports = MAP;` (a map module as is) |
| `.wasm`   | the name without `.wasm` (`main_bg`) | `{"binary": BASE64}`                         |

so that `require("main.js.map")` returns the source map. Every other entry
is skipped and printed. Two files of one module name are an error, and so
is a directory without a module: an upload replaces every module of a branch.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
