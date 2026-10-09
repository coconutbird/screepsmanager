//! The modules of a build directory. A file directly in the directory is a
//! module when its name ends in
//!
//! - `.js`: the JavaScript module of the name without `.js` (`main.js`:
//!   `main`);
//! - `.js.map`: the source map module of the whole name (`main.js.map`), so
//!   that `require("main.js.map")` returns the map. A map that is JSON
//!   (its text starts with `{`) is uploaded as `module.exports = MAP;`, a
//!   map that is a module already as it is;
//! - `.wasm`: the binary module of the name without `.wasm`
//!   (`main_bg.wasm`: `main_bg`), uploaded as base64.
//!
//! Every other entry is skipped. Two files of one module name are an
//! error, and so is a directory without a module: an upload replaces every
//! module of the branch.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

/// The kind of a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// JavaScript, uploaded as text.
    Js,
    /// A source map, uploaded as JavaScript text that exports it.
    SourceMap,
    /// WebAssembly, uploaded as `{"binary": BASE64}`.
    Wasm,
}

impl Kind {
    /// The kind and the module name of the file `file_name`, or `None` when
    /// the file is not a module.
    fn of(file_name: &str) -> Option<(Self, &str)> {
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
        Some((
            kind,
            if kind == Self::SourceMap {
                file_name
            } else {
                stem
            },
        ))
    }

    /// The name of the kind, for output.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Js => "js",
            Self::SourceMap => "source map",
            Self::Wasm => "wasm",
        }
    }

    /// Whether the module is a binary module.
    pub(crate) fn is_binary(self) -> bool {
        self == Self::Wasm
    }

    /// The code of a module of this kind with the file content `bytes`: the
    /// text of a JavaScript or source map module, the base64 of a binary
    /// module.
    fn code(self, bytes: Vec<u8>) -> Result<String, String> {
        let text = |bytes| String::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_owned());
        match self {
            Self::Js => text(bytes),
            Self::SourceMap => {
                let map = text(bytes)?;
                Ok(if map.trim_start().starts_with('{') {
                    format!("module.exports = {map};")
                } else {
                    map
                })
            }
            Self::Wasm => Ok(BASE64.encode(bytes)),
        }
    }
}

/// A module of a build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Module {
    /// The name of the module in the branch: what `require` takes.
    pub(crate) name: String,
    /// The kind of the module.
    pub(crate) kind: Kind,
    /// The text of a JavaScript or source map module, the base64 of a
    /// binary module.
    pub(crate) code: String,
    /// The file of the module.
    pub(crate) path: PathBuf,
    /// The bytes of the file.
    pub(crate) len: usize,
}

/// An entry of the build directory that is not a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Skipped {
    /// The entry.
    pub(crate) path: PathBuf,
    /// Why it is not a module.
    pub(crate) reason: &'static str,
}

/// The modules of a build directory and the entries that it skipped, each
/// in path order.
#[derive(Debug, Default)]
pub(crate) struct Build {
    /// The modules.
    pub(crate) modules: Vec<Module>,
    /// The entries that are not modules.
    pub(crate) skipped: Vec<Skipped>,
}

impl Build {
    /// Adds `module`.
    fn push(&mut self, module: Module) -> Result<(), String> {
        if let Some(other) = self.modules.iter().find(|other| other.name == module.name) {
            return Err(format!(
                "{} and {} are both the module {:?}",
                other.path.display(),
                module.path.display(),
                module.name
            ));
        }
        self.modules.push(module);
        Ok(())
    }
}

/// The modules of the directory `dir` (see the module documentation).
///
/// # Errors
///
/// When the directory or a module file is not readable, a JavaScript or
/// source map file is not UTF-8, two files are one module, or the directory
/// has no module.
pub(crate) fn read(dir: &Path) -> Result<Build, String> {
    let mut paths = std::fs::read_dir(dir)
        .and_then(|entries| {
            entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<PathBuf>, _>>()
        })
        .map_err(|error| io_error(dir, &error))?;
    paths.sort();
    let mut build = Build::default();
    for path in paths {
        let metadata = std::fs::metadata(&path).map_err(|error| io_error(&path, &error))?;
        if !metadata.is_file() {
            build.skipped.push(Skipped {
                path,
                reason: "not a file",
            });
            continue;
        }
        let Some((kind, name)) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(Kind::of)
        else {
            build.skipped.push(Skipped {
                path,
                reason: "not a .js, .js.map, or .wasm file",
            });
            continue;
        };
        let name = name.to_owned();
        let bytes = std::fs::read(&path).map_err(|error| io_error(&path, &error))?;
        let len = bytes.len();
        let code = kind
            .code(bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        build.push(Module {
            name,
            kind,
            code,
            path,
            len,
        })?;
    }
    if build.modules.is_empty() {
        return Err(format!(
            "{}: no .js, .js.map, or .wasm file, and an upload replaces every module of the branch",
            dir.display()
        ));
    }
    Ok(build)
}

/// The error of the I/O `error` on `path`.
fn io_error(path: &Path, error: &std::io::Error) -> String {
    format!("{}: {error}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Module names come from the suffixes; a name of only a suffix and
    /// other suffixes are not modules.
    #[test]
    fn module_names() {
        assert_eq!(Kind::of("main.js"), Some((Kind::Js, "main")));
        assert_eq!(Kind::of("main.min.js"), Some((Kind::Js, "main.min")));
        assert_eq!(
            Kind::of("main.js.map"),
            Some((Kind::SourceMap, "main.js.map"))
        );
        assert_eq!(Kind::of("main_bg.wasm"), Some((Kind::Wasm, "main_bg")));
        for other in [
            ".js",
            ".js.map",
            ".wasm",
            "main.mjs",
            "main.d.ts",
            "main.js.LICENSE.txt",
        ] {
            assert_eq!(Kind::of(other), None, "{other}");
        }
    }

    /// A JSON map becomes a module that exports it; a map module stays as
    /// it is; binaries are base64; JavaScript must be UTF-8.
    #[test]
    fn module_code() {
        assert_eq!(
            Kind::SourceMap.code(br#"{"version":3}"#.to_vec()),
            Ok(r#"module.exports = {"version":3};"#.to_owned())
        );
        let wrapped = r#"module.exports = {"version":3};"#;
        assert_eq!(
            Kind::SourceMap.code(wrapped.as_bytes().to_vec()),
            Ok(wrapped.to_owned())
        );
        assert_eq!(
            Kind::Wasm.code(b"\0asm".to_vec()),
            Ok("AGFzbQ==".to_owned())
        );
        assert!(Kind::Js.code(vec![0xff]).is_err());
    }

    /// Two files of one module name are an error that names both.
    #[test]
    fn one_file_per_module() {
        let module = |file: &str, kind| Module {
            name: "main".to_owned(),
            kind,
            code: String::new(),
            path: PathBuf::from(file),
            len: 0,
        };
        let mut build = Build::default();
        assert_eq!(build.push(module("main.js", Kind::Js)), Ok(()));
        let error = build
            .push(module("main.wasm", Kind::Wasm))
            .expect_err("a second main");
        assert!(
            error.contains("main.js") && error.contains("main.wasm"),
            "{error}"
        );
    }
}
