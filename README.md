# screepsmanager

Uploads built Screeps code to the branches of Screeps servers: the official
server, seasonal servers, and private servers. A configuration names the
servers and the profiles that upload to them; one command uploads a build to
one profile or to several. It reads a build directory that any tool made
(Rollup, esbuild, `wasm-pack`, ...), so the build needs no plugin and no
Node.js for the upload.

## Build

```sh
cargo build --release   # target/release/screepsmanager
mise run ci             # fmt, clippy (pedantic, -D warnings), tests
```

The toolchain is pinned in `rust-toolchain.toml` (and in `mise.toml`, whose
`RUSTUP_TOOLCHAIN` would override it).

## Usage

```sh
screepsmanager profiles                          # the profiles of the configuration
screepsmanager upload                            # the default profiles
screepsmanager upload -p main                    # one profile
screepsmanager upload -p main,season             # several, in order
screepsmanager upload -p dev --branch auto       # the current git branch
screepsmanager upload -p main --activate world   # and make the branch run in the world
screepsmanager upload --dry-run                  # check everything but the server
```

Both commands read the nearest `screepsmanager.toml` in the working
directory or a parent (`--config FILE` or `$SCREEPSMANAGER_CONFIG` name
another). `upload [DIR]` reads the modules of the build directory (by
default the `dir` of the configuration) once, then for each profile replaces
every module of its branch, creating a branch that the account does not
have, and makes the branch run where the profile and `--activate` say.

Without `--profile` (or `$SCREEPSMANAGER_PROFILE`), `upload` takes the
`default` profiles of the configuration, or its only profile. Profiles of one
server share one sign-in. The first failure stops the upload: its error goes
to stderr as `screepsmanager: ...` with status 1 (2 for a bad command line).

```text
config /home/me/bot/screepsmanager.toml
build /home/me/bot/dist: 3 modules
  main         js                 39 bytes  main.js
  main.js.map  source map         37 bytes  main.js.map
  main_bg      wasm                8 bytes  main_bg.wasm
profile main: branch main on https://screeps.com/, run in world
  uploaded 3 modules
  branch main already runs in world
profile season: branch default on https://screeps.com/season/
  created branch default
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

A server signs in with `token`, or with `email` and `password`. A secret is a
string, or `{ env = "VARIABLE" }` to read it from the environment when an
upload needs it, so that the configuration can be committed. Names have
letters, digits, `-`, `_`, and `.`. A key that the format does not have is an
error.

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
