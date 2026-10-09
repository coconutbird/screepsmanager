//! The modules of a build directory. A file directly in the directory is a
//! module when its name ends in
//!
//! - `.js`: the JavaScript module of the name without `.js` (`main.js`:
//!   `main`);
//! - `.js.map`: the source map module of the whole name (`main.js.map`), so
//!   that `require("main.js.map")` returns the map. A map that is JSON (its
//!   text starts with `{`) is uploaded as `module.exports = MAP;`, a map
//!   that is a module already as it is;
//! - `.wasm`: the binary module of the name without `.wasm`
//!   (`main_bg.wasm`: `main_bg`), uploaded as base64.
//!
//! Every other entry is skipped. Two files of one module name are an
//! error, and so is a directory without a module: an upload replaces every
//! module of a branch.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt;
use std::path::{Path, PathBuf};
use std::string::FromUtf8Error;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::ser::{SerializeMap as _, SerializeStruct as _};
use serde::{Serialize, Serializer};

/// The name of a module: what `require` takes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub(crate) struct ModuleName(String);

impl ModuleName {
    /// The name.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

/// The code of a module as the API takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Code {
    /// JavaScript text.
    Js(String),
    /// A source map as JavaScript text that exports it.
    SourceMap(String),
    /// WebAssembly as base64, uploaded as `{"binary": BASE64}`.
    Wasm(String),
}

impl Code {
    /// The kind of the code, for output.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Js(_) => "js",
            Self::SourceMap(_) => "source map",
            Self::Wasm(_) => "wasm",
        }
    }
}

impl Serialize for Code {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Js(text) | Self::SourceMap(text) => serializer.serialize_str(text),
            Self::Wasm(base64) => {
                let mut binary = serializer.serialize_struct("Binary", 1)?;
                binary.serialize_field("binary", base64)?;
                binary.end()
            }
        }
    }
}

/// A module of a build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Module {
    /// The code.
    pub(crate) code: Code,
    /// The file.
    pub(crate) file: PathBuf,
    /// The bytes of the file.
    pub(crate) len: usize,
}

/// The modules of a build by name. They serialize as the `modules` of the
/// API: each name to its code.
#[derive(Debug, Default)]
pub(crate) struct Modules(BTreeMap<ModuleName, Module>);

impl Modules {
    /// The number of modules.
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there is no module.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The modules in name order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&ModuleName, &Module)> {
        self.0.iter()
    }

    /// Adds the module `name`.
    fn insert(&mut self, name: ModuleName, module: Module) -> Result<(), Error> {
        match self.0.entry(name) {
            Entry::Occupied(entry) => Err(Error::Duplicate {
                first: entry.get().file.clone(),
                second: module.file,
                name: entry.key().clone(),
            }),
            Entry::Vacant(entry) => {
                entry.insert(module);
                Ok(())
            }
        }
    }
}

impl Serialize for Modules {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, module) in &self.0 {
            map.serialize_entry(name, &module.code)?;
        }
        map.end()
    }
}

/// Why an entry of the build directory is not a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkipReason {
    /// A directory or another entry that is not a file.
    NotAFile,
    /// A file whose name does not end in a module suffix.
    NotAModule,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotAFile => "not a file",
            Self::NotAModule => "not a .js, .js.map, or .wasm file",
        })
    }
}

/// An entry of the build directory that is not a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Skipped {
    /// The entry.
    pub(crate) path: PathBuf,
    /// Why it is not a module.
    pub(crate) reason: SkipReason,
}

/// The modules of a build directory, and the entries that it skipped in
/// path order.
#[derive(Debug, Default)]
pub(crate) struct Build {
    /// The modules.
    pub(crate) modules: Modules,
    /// The entries that are not modules.
    pub(crate) skipped: Vec<Skipped>,
}

/// The modules of the directory `dir` (see the module documentation).
///
/// # Errors
///
/// When the directory or a module file is not readable, a JavaScript or
/// source map file is not UTF-8, two files are one module, or the directory
/// has no module.
pub(crate) fn read(dir: &Path) -> Result<Build, Error> {
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| Error::Io { path, source }
    };
    let mut paths = std::fs::read_dir(dir)
        .and_then(|entries| {
            entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<PathBuf>, _>>()
        })
        .map_err(io(dir))?;
    paths.sort();
    let mut build = Build::default();
    for path in paths {
        let is_file = match std::fs::metadata(&path) {
            Ok(metadata) => metadata.is_file(),
            Err(source) => return Err(Error::Io { path, source }),
        };
        let module = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(Kind::of);
        let (kind, name) = match (is_file, module) {
            (true, Some(module)) => module,
            (false, _) => {
                build.skipped.push(Skipped {
                    path,
                    reason: SkipReason::NotAFile,
                });
                continue;
            }
            (true, None) => {
                build.skipped.push(Skipped {
                    path,
                    reason: SkipReason::NotAModule,
                });
                continue;
            }
        };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) => return Err(Error::Io { path, source }),
        };
        let len = bytes.len();
        let code = match kind.code(bytes) {
            Ok(code) => code,
            Err(source) => return Err(Error::NotUtf8 { path, source }),
        };
        build.modules.insert(
            name,
            Module {
                code,
                file: path,
                len,
            },
        )?;
    }
    if build.modules.is_empty() {
        return Err(Error::Empty {
            dir: dir.to_owned(),
        });
    }
    Ok(build)
}

/// The kind of a module file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `.js`.
    Js,
    /// `.js.map`.
    SourceMap,
    /// `.wasm`.
    Wasm,
}

impl Kind {
    /// The kind and the module name of the file `file_name`, or `None` when
    /// the file is not a module.
    fn of(file_name: &str) -> Option<(Self, ModuleName)> {
        let (kind, stem) = [
            (".js.map", Self::SourceMap),
            (".js", Self::Js),
            (".wasm", Self::Wasm),
        ]
        .into_iter()
        .find_map(|(suffix, kind)| Some((kind, file_name.strip_suffix(suffix)?)))?;
        if stem.is_empty() {
            return None;
        }
        let name = if kind == Self::SourceMap {
            file_name
        } else {
            stem
        };
        Some((kind, ModuleName(name.to_owned())))
    }

    /// The code of a module of this kind with the file content `bytes`.
    fn code(self, bytes: Vec<u8>) -> Result<Code, FromUtf8Error> {
        Ok(match self {
            Self::Js => Code::Js(String::from_utf8(bytes)?),
            Self::SourceMap => {
                let map = String::from_utf8(bytes)?;
                Code::SourceMap(if map.trim_start().starts_with('{') {
                    format!("module.exports = {map};")
                } else {
                    map
                })
            }
            Self::Wasm => Code::Wasm(BASE64.encode(bytes)),
        })
    }
}

/// A build directory that does not read as modules.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// An entry is not readable.
    #[error("{}: {source}", .path.display())]
    Io {
        /// The entry.
        path: PathBuf,
        /// The read error.
        source: std::io::Error,
    },
    /// A JavaScript or source map file is not UTF-8.
    #[error("{}: not UTF-8 text ({source})", .path.display())]
    NotUtf8 {
        /// The file.
        path: PathBuf,
        /// Where the text is not UTF-8.
        source: FromUtf8Error,
    },
    /// Two files are one module.
    #[error("{} and {} are both the module {name}", .first.display(), .second.display())]
    Duplicate {
        /// The module.
        name: ModuleName,
        /// The first file, in path order.
        first: PathBuf,
        /// The second file.
        second: PathBuf,
    },
    /// The directory has no module.
    #[error(
        "{}: no .js, .js.map, or .wasm file, and an upload replaces every module of a branch",
        .dir.display()
    )]
    Empty {
        /// The directory.
        dir: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The module name of `file`, if it is a module.
    fn name(file: &str) -> Option<(Kind, String)> {
        Kind::of(file).map(|(kind, name)| (kind, name.to_string()))
    }

    /// Module names come from the suffixes; a name of only a suffix and
    /// other suffixes are not modules.
    #[test]
    fn module_names() {
        assert_eq!(name("main.js"), Some((Kind::Js, "main".to_owned())));
        assert_eq!(name("main.min.js"), Some((Kind::Js, "main.min".to_owned())));
        assert_eq!(
            name("main.js.map"),
            Some((Kind::SourceMap, "main.js.map".to_owned()))
        );
        assert_eq!(
            name("main_bg.wasm"),
            Some((Kind::Wasm, "main_bg".to_owned()))
        );
        for other in [
            ".js",
            ".js.map",
            ".wasm",
            "main.mjs",
            "main.d.ts",
            "main.js.LICENSE.txt",
        ] {
            assert_eq!(name(other), None, "{other}");
        }
    }

    /// A JSON map becomes a module that exports it; a map module stays as
    /// it is; binaries are base64; JavaScript must be UTF-8.
    #[test]
    fn module_code() {
        assert_eq!(
            Kind::SourceMap.code(br#"{"version":3}"#.to_vec()),
            Ok(Code::SourceMap(
                r#"module.exports = {"version":3};"#.to_owned()
            ))
        );
        let wrapped = r#"module.exports = {"version":3};"#;
        assert_eq!(
            Kind::SourceMap.code(wrapped.as_bytes().to_vec()),
            Ok(Code::SourceMap(wrapped.to_owned()))
        );
        assert_eq!(
            Kind::Wasm.code(b"\0asm".to_vec()),
            Ok(Code::Wasm("AGFzbQ==".to_owned()))
        );
        assert!(Kind::Js.code(vec![0xff]).is_err());
    }

    /// Two files of one module name are an error that names both; modules
    /// serialize as each name to its code, binaries as `{"binary": ...}`.
    #[test]
    fn modules() {
        let module = |file: &str, code| Module {
            code,
            file: PathBuf::from(file),
            len: 0,
        };
        let name = |name: &str| ModuleName(name.to_owned());
        let mut modules = Modules::default();
        let inserted = [
            ("main", module("main.js", Code::Js("loop".to_owned()))),
            (
                "main.js.map",
                module("main.js.map", Code::SourceMap("map".to_owned())),
            ),
            (
                "main_bg",
                module("main_bg.wasm", Code::Wasm("AGFzbQ==".to_owned())),
            ),
        ]
        .into_iter()
        .map(|(module_name, module)| modules.insert(name(module_name), module))
        .collect::<Result<Vec<()>, Error>>();
        assert!(inserted.is_ok(), "{inserted:?}");
        assert_eq!(
            serde_json::to_string(&modules).map_err(|error| error.to_string()),
            Ok(r#"{"main":"loop","main.js.map":"map","main_bg":{"binary":"AGFzbQ=="}}"#.to_owned())
        );
        let error = modules
            .insert(name("main"), module("main.wasm", Code::Wasm(String::new())))
            .expect_err("a second main");
        let error = error.to_string();
        assert!(
            error.contains("main.js") && error.contains("main.wasm"),
            "{error}"
        );
    }
}
