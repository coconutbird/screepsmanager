# screepmanager

Uploads built Screeps code to a branch of a Screeps server: the official
server, a seasonal server, or a private server. It reads a build directory
that any tool made (Rollup, esbuild, `wasm-pack`, ...), so the build needs no
plugin and no Node.js for the upload.

The configuration file is the one of
[`@coconutbird/plugin-screeps-rollup`](https://github.com/coconutbird/plugin-screeps-rollup).

## Build

```sh
cargo build --release   # target/release/screepmanager
mise run ci             # fmt, clippy (pedantic, -D warnings), tests
```

The toolchain is pinned in `rust-toolchain.toml`.

## Usage

```sh
screepmanager upload dist --target main                      # the branch of the destination
screepmanager upload dist --target main --branch default     # another branch
screepmanager upload dist --target pserver --branch auto     # the current git branch
screepmanager upload dist --target main --activate world     # and make it the branch that runs
screepmanager upload dist --target main --dry-run            # print what would go, upload nothing
```

`upload DIR --target NAME` takes the destination NAME of the configuration
file (`--config FILE`, default `screeps.config.json`), reads the modules of
DIR, and replaces every module of the branch with them. When the account
has no branch of that name, the upload creates it. `--activate world` or
`--activate sim` (repeatable) then makes the branch the one that runs in the
world or in the simulator; without it, the active branches stay as they
are. Each step prints one line; an error prints `screepmanager: ERROR` to
stderr and exits with status 1 (2 for a bad command line).

### Modules

A file directly in DIR is a module when its name ends in

| Suffix    | Module                                   | Uploaded as                                   |
| --------- | ---------------------------------------- | --------------------------------------------- |
| `.js`     | the name without `.js` (`main`)          | text                                          |
| `.js.map` | the whole name (`main.js.map`)           | `module.exports = MAP;` (a map module as is)  |
| `.wasm`   | the name without `.wasm` (`main_bg`)     | `{"binary": BASE64}`                          |

so that `require("main.js.map")` returns the source map. Every other entry
is skipped and printed. Two files of one module name are an error, and so
is a directory without a module.

## Configuration

```json
{
  "main": {
    "token": "<TOKEN>",
    "branch": "main"
  },
  "season": {
    "token": "<TOKEN>",
    "path": "/season"
  },
  "pserver": {
    "email": "name@server.tld",
    "password": "<PASSWORD>",
    "protocol": "http",
    "hostname": "127.0.0.1",
    "port": 21025,
    "branch": "auto"
  }
}
```

| Key        | Default                       | Meaning                                                         |
| ---------- | ----------------------------- | --------------------------------------------------------------- |
| `token`    | -                             | API token. When set, `email` and `password` are not used        |
| `email`    | -                             | Account email, needed without `token`                           |
| `password` | -                             | Account password, needed without `token` (screepsmod-auth)      |
| `protocol` | `https`                       | `http` or `https`                                               |
| `hostname` | `screeps.com`                 | Server host                                                     |
| `port`     | the default of the protocol   | Server port; a private server usually needs `21025`             |
| `path`     | `/`                           | API path; `/season` for the seasonal server                     |
| `branch`   | `default`                     | Branch to upload to; `auto` is the current git branch           |

A key that the format does not have is an error, so that a misspelt key
does not upload to the official server. Unlike the Rollup plugin, an unset
`port` is the default of the protocol (443 for `https`) rather than `21025`.

Keep `screeps.config.json` out of version control: it holds credentials.
