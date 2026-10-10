//! `--env-file`: a dotenv file of the variables that `{ env = "VARIABLE" }`
//! secrets read. A variable of the process environment wins over the file,
//! so that a shell or CI can override it. The values are secrets: neither
//! they nor the lines of the file appear in output or errors.
//!
//! The file only supplies secrets: the manager does not change its own
//! environment, so subprocesses (the spawn selector) do not inherit it.

use std::collections::BTreeMap;
use std::env::VarError;
use std::path::{Path, PathBuf};

use crate::config::Sensitive;

/// The bytes of a UTF-8 byte order mark, which editors on Windows write.
const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The variables of a dotenv file, or none without one.
#[derive(Debug, Default)]
pub(crate) struct EnvFile {
    /// The file, if one was given.
    pub(crate) path: Option<PathBuf>,
    /// The variables: the first definition of a name wins, as dotenv tools
    /// do.
    vars: BTreeMap<String, Sensitive>,
}

impl EnvFile {
    /// The variables of the file `path`, or none without a path.
    ///
    /// # Errors
    ///
    /// When the file is not readable or not a valid dotenv file.
    pub(crate) fn load(path: Option<&Path>) -> Result<Self, Error> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let bytes = std::fs::read(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })?;
        let vars = parse(&bytes).map_err(|reason| Error::Parse {
            path: path.to_owned(),
            reason,
        })?;
        Ok(Self {
            path: Some(path.to_owned()),
            vars,
        })
    }

    /// The number of variables of the file.
    pub(crate) fn len(&self) -> usize {
        self.vars.len()
    }

    /// The value of the variable `name`: from the process environment
    /// `process` (`std::env::var`), or else from the file.
    ///
    /// # Errors
    ///
    /// When the process has the variable with a value that is not Unicode.
    pub(crate) fn var(
        &self,
        name: &str,
        process: &dyn Fn(&str) -> Result<String, VarError>,
    ) -> Result<Option<Sensitive>, VarError> {
        match process(name) {
            Ok(value) => Ok(Some(Sensitive::new(value))),
            Err(VarError::NotPresent) => Ok(self.vars.get(name).cloned()),
            Err(error) => Err(error),
        }
    }

    /// The variables `vars`, as a file would have them.
    #[cfg(test)]
    pub(crate) fn of(vars: &[(&str, &str)]) -> Self {
        Self {
            path: None,
            vars: vars
                .iter()
                .map(|&(name, value)| (name.to_owned(), Sensitive::new(value.to_owned())))
                .collect(),
        }
    }
}

/// The variables of the dotenv text `bytes`.
fn parse(bytes: &[u8]) -> Result<BTreeMap<String, Sensitive>, ParseError> {
    let bytes = bytes.strip_prefix(BOM).unwrap_or(bytes);
    let mut vars = BTreeMap::new();
    for item in dotenvy::from_read_iter(bytes) {
        let (name, value) = item.map_err(|error| ParseError::of(&error, bytes))?;
        vars.entry(name).or_insert_with(|| Sensitive::new(value));
    }
    Ok(vars)
}

/// Why a dotenv file does not parse, without its text: a line may hold a
/// secret.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ParseError {
    /// A line (from the line `line`, when it is known) is not
    /// `NAME=VALUE`, or a quote does not close.
    #[error(
        "{}is not NAME=VALUE, or a quote does not close (its text is not shown)",
        line_text(.line.as_ref())
    )]
    Line {
        /// The number of the first line of the definition, from 1.
        line: Option<usize>,
    },
    /// The file is not readable as text.
    #[error("not UTF-8 text")]
    Text,
    /// A `${VARIABLE}` substitution refers to a variable of the process
    /// whose value is not Unicode.
    #[error("a substitution refers to a variable that is not Unicode")]
    Substitution,
}

impl ParseError {
    /// The error of dotenvy `error` in the file text `bytes`.
    fn of(error: &dotenvy::Error, bytes: &[u8]) -> Self {
        match error {
            dotenvy::Error::LineParse(text, _) => Self::Line {
                line: line_of(bytes, text),
            },
            dotenvy::Error::EnvVar(_) => Self::Substitution,
            // An error of reading a byte slice is invalid UTF-8.
            _ => Self::Text,
        }
    }
}

/// The number of the line, from 1, where the definition `text` starts in
/// `bytes`. Parsing is per definition, so the first definition of the
/// same text is the one that failed.
fn line_of(bytes: &[u8], text: &str) -> Option<usize> {
    let text = text.trim_end_matches(['\r', '\n']);
    if text.is_empty() {
        return None;
    }
    let start = bytes
        .windows(text.len())
        .position(|window| window == text.as_bytes())?;
    Some(bytes[..start].split(|&byte| byte == b'\n').count())
}

/// `line N ` for the line `line`, or `a line ` when it is not known.
fn line_text(line: Option<&usize>) -> String {
    line.map_or_else(|| "a line ".to_owned(), |line| format!("line {line} "))
}

/// An env file that is not available or not valid.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The file is not readable.
    #[error("env file {}: {source}", .path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// The read error.
        source: std::io::Error,
    },
    /// The file is not a valid dotenv file.
    #[error("env file {}: {reason}", .path.display())]
    Parse {
        /// The file.
        path: PathBuf,
        /// Why.
        reason: ParseError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A process environment without variables.
    fn none(_: &str) -> Result<String, VarError> {
        Err(VarError::NotPresent)
    }

    /// Quotes, comments, `export`, a byte order mark, and substitutions
    /// parse; the first definition of a name wins.
    #[test]
    fn parses() {
        let text = b"\xEF\xBB\xBF# secrets\nSCREEPS_TOKEN=abc\nexport QUOTED=\"a b\"\nSINGLE='x#y'\nSCREEPS_TOKEN=second\nJOINED=${SINGLE}z\n";
        let vars = parse(text).expect("a valid file");
        let get = |name: &str| vars.get(name).map(|value| value.expose().to_owned());
        assert_eq!(get("SCREEPS_TOKEN").as_deref(), Some("abc"));
        assert_eq!(get("QUOTED").as_deref(), Some("a b"));
        assert_eq!(get("SINGLE").as_deref(), Some("x#y"));
        assert_eq!(get("JOINED").as_deref(), Some("x#yz"));
    }

    /// A line that does not parse names its line, not its text.
    #[test]
    fn malformed() {
        let error = parse(b"A=1\n\nTOKEN=\"secret-value\n").expect_err("an open quote");
        assert_eq!(error, ParseError::Line { line: Some(3) });
        let text = error.to_string();
        assert!(text.starts_with("line 3 "), "{text}");
        assert!(!text.contains("secret-value"), "{text}");

        let error = parse(b"NOT A DEFINITION secret-value\n").expect_err("no =");
        assert!(!error.to_string().contains("secret-value"), "{error}");
        assert_eq!(parse(b"A=\xFF\n"), Err(ParseError::Text));
    }

    /// The process environment wins over the file, even when it is empty;
    /// the file fills in what the process does not have.
    #[test]
    fn precedence() {
        let file = EnvFile::of(&[("A", "file"), ("B", "file")]);
        let process = |name: &str| match name {
            "A" => Ok("process".to_owned()),
            "E" => Ok(String::new()),
            _ => Err(VarError::NotPresent),
        };
        let value = |name| {
            file.var(name, &process)
                .expect("Unicode")
                .map(|value| value.expose().to_owned())
        };
        assert_eq!(value("A").as_deref(), Some("process"));
        assert_eq!(value("B").as_deref(), Some("file"));
        assert_eq!(value("E").as_deref(), Some(""));
        assert_eq!(value("C"), None);
        assert_eq!(EnvFile::default().var("A", &none), Ok(None));
    }

    /// A missing file is an error, not an empty file.
    #[test]
    fn missing() {
        let error = EnvFile::load(Some(Path::new("no/such/dir/.env"))).expect_err("missing");
        assert!(matches!(error, Error::Read { .. }), "{error}");
        assert_eq!(EnvFile::load(None).map(|file| file.len()).ok(), Some(0));
    }
}
