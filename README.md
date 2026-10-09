# screepsmanager

Uploads built Screeps code to the branches of Screeps servers: the official
server, seasonal servers, and private servers. A configuration names the
servers and the profiles that upload to them; one command uploads a build to
one profile or to several. It reads a build directory that any tool made
(Rollup, esbuild, `wasm-pack`, ...), so the build needs no plugin and no
Node.js for the upload.

## Install

With [mise](https://mise.jdx.dev), from the prebuilt binaries of the GitHub
releases (Linux with glibc 2.35 or newer and macOS, each on x86-64 and arm64;
Windows on x86-64):

```sh
mise use -g github:coconutbird/screepsmanager          # the latest release, for every directory
mise use github:coconutbird/screepsmanager@0.1.0       # pinned in this project's mise.toml
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

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
