//! Secrets of the configuration: a value written in the file, or the name
//! of a variable that the process environment or the `--env-file` sets,
//! read only when a command needs it. Debug output never shows a value.

use std::fmt;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::envfile::EnvFile;

/// A secret value. Its debug output does not show it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Sensitive(String);

impl Sensitive {
    /// The secret `value`.
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    /// The value.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Sensitive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sensitive(..)")
    }
}

/// A secret of a server: written in the file, or read from an environment
/// variable when an upload needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Secret {
    /// The value, from the file.
    Value(Sensitive),
    /// The environment variable of the value.
    Env(String),
}

impl Secret {
    /// The value of the secret: a variable comes from the process
    /// environment, or else from the `--env-file` variables `env`.
    ///
    /// # Errors
    ///
    /// When the environment variable is not set, not Unicode, or empty.
    pub(crate) fn resolve(&self, env: &EnvFile) -> Result<Sensitive, SecretError> {
        self.resolve_with(env, &|name: &str| std::env::var(name))
    }

    /// The value of the secret, with `process` for the process environment.
    fn resolve_with(
        &self,
        env: &EnvFile,
        process: &dyn Fn(&str) -> Result<String, std::env::VarError>,
    ) -> Result<Sensitive, SecretError> {
        match self {
            Self::Value(value) => Ok(value.clone()),
            Self::Env(variable) => match env.var(variable, process) {
                Ok(Some(value)) if value.expose().is_empty() => {
                    Err(SecretError::Empty(variable.clone()))
                }
                Ok(Some(value)) => Ok(value),
                Ok(None) => Err(SecretError::NotSet(variable.clone())),
                Err(_) => Err(SecretError::NotUnicode(variable.clone())),
            },
        }
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(_) => f.write_str("in the file"),
            Self::Env(variable) => write!(f, "from ${variable}"),
        }
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The table form of a secret.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FromEnv {
            env: String,
        }

        /// Reads a string or a `{ env = "VARIABLE" }` table.
        struct SecretVisitor;

        impl<'de> Visitor<'de> for SecretVisitor {
            type Value = Secret;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string, or a table { env = \"VARIABLE\" }")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Secret, E> {
                if value.is_empty() {
                    return Err(E::custom("a secret is not empty"));
                }
                Ok(Secret::Value(Sensitive(value.to_owned())))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Secret, A::Error> {
                let FromEnv { env } =
                    FromEnv::deserialize(de::value::MapAccessDeserializer::new(map))?;
                if env.is_empty() {
                    return Err(de::Error::custom(
                        "an environment variable name is not empty",
                    ));
                }
                Ok(Secret::Env(env))
            }
        }

        deserializer.deserialize_any(SecretVisitor)
    }
}

/// A secret that is not available.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SecretError {
    /// The variable is not set, in the environment or the env file.
    #[error("${0} is not set (in the environment or --env-file)")]
    NotSet(String),
    /// The value of the variable is not Unicode.
    #[error("${0} is not Unicode")]
    NotUnicode(String),
    /// The value of the variable is empty.
    #[error("${0} is empty")]
    Empty(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A secret variable comes from the process, or else from the env
    /// file; empty and missing values are errors that name the variable.
    #[test]
    fn secrets() {
        let file = EnvFile::of(&[("FILE", "from-file"), ("BOTH", "from-file")]);
        let process = |name: &str| match name {
            "BOTH" => Ok("from-process".to_owned()),
            "EMPTY" => Ok(String::new()),
            _ => Err(std::env::VarError::NotPresent),
        };
        let resolve = |name: &str| {
            Secret::Env(name.to_owned())
                .resolve_with(&file, &process)
                .map(|value| value.expose().to_owned())
        };
        assert_eq!(resolve("FILE").as_deref(), Ok("from-file"));
        assert_eq!(resolve("BOTH").as_deref(), Ok("from-process"));
        assert_eq!(
            resolve("EMPTY"),
            Err(SecretError::Empty("EMPTY".to_owned()))
        );
        assert_eq!(resolve("NONE"), Err(SecretError::NotSet("NONE".to_owned())));
        let value = Secret::Value(Sensitive::new("v".to_owned()));
        assert_eq!(
            value.resolve_with(&EnvFile::default(), &process),
            Ok(Sensitive::new("v".to_owned()))
        );
    }
}
