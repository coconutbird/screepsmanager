//! Branches of a Screeps account: their names, `auto` (the current git
//! branch), and where a branch runs.

use std::fmt;
use std::process::Command;
use std::str::FromStr;

use serde::{Deserialize, Serialize, Serializer};

/// The branch that `auto` names.
const AUTO: &str = "auto";
/// The branch of a profile that names none: the first branch of every
/// Screeps account.
const DEFAULT: &str = "default";

/// The name of a branch: not empty, without white space around it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BranchName(String);

impl BranchName {
    /// The name as the API takes it.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for BranchName {
    type Error = InvalidBranch;

    fn try_from(name: String) -> Result<Self, InvalidBranch> {
        if name.is_empty() || name.trim() != name {
            return Err(InvalidBranch(name));
        }
        Ok(Self(name))
    }
}

impl FromStr for BranchName {
    type Err = InvalidBranch;

    fn from_str(name: &str) -> Result<Self, InvalidBranch> {
        Self::try_from(name.to_owned())
    }
}

impl fmt::Display for BranchName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

impl Serialize for BranchName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// A branch name that is empty or has white space around it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("branch {0:?}: a branch name is not empty and has no white space around it")]
pub(crate) struct InvalidBranch(String);

/// The branch of a profile or of `--branch`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(crate) enum Branch {
    /// The current branch of the git repository of the working directory.
    Auto,
    /// The branch of this name.
    Named(BranchName),
}

impl Branch {
    /// The name of the branch: its own, or for `auto` the current git
    /// branch, which `current` keeps for the next call.
    ///
    /// # Errors
    ///
    /// For `auto`, when git does not name a current branch.
    pub(crate) fn resolve(&self, current: &mut Option<BranchName>) -> Result<BranchName, GitError> {
        match self {
            Self::Named(name) => Ok(name.clone()),
            Self::Auto => {
                if let Some(name) = current {
                    return Ok(name.clone());
                }
                let name = git_branch()?;
                *current = Some(name.clone());
                Ok(name)
            }
        }
    }
}

impl Default for Branch {
    fn default() -> Self {
        Self::Named(BranchName(DEFAULT.to_owned()))
    }
}

impl TryFrom<String> for Branch {
    type Error = InvalidBranch;

    fn try_from(name: String) -> Result<Self, InvalidBranch> {
        if name == AUTO {
            Ok(Self::Auto)
        } else {
            BranchName::try_from(name).map(Self::Named)
        }
    }
}

impl FromStr for Branch {
    type Err = InvalidBranch;

    fn from_str(name: &str) -> Result<Self, InvalidBranch> {
        Self::try_from(name.to_owned())
    }
}

impl fmt::Display for Branch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.pad(AUTO),
            Self::Named(name) => name.fmt(f),
        }
    }
}

/// Where a branch runs. Each account has one branch that runs in the world
/// and one that runs in the simulator.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Active {
    /// The world: the shards of the server.
    World,
    /// The simulator.
    Sim,
}

impl Active {
    /// Every place, in output order.
    pub(crate) const ALL: [Self; 2] = [Self::World, Self::Sim];

    /// The `activeName` of the API.
    pub(crate) fn api_name(self) -> &'static str {
        match self {
            Self::World => "activeWorld",
            Self::Sim => "activeSim",
        }
    }
}

impl fmt::Display for Active {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::World => "world",
            Self::Sim => "sim",
        })
    }
}

/// Why `auto` names no branch.
#[derive(Debug, thiserror::Error)]
pub(crate) enum GitError {
    /// git does not run.
    #[error("git: {0}")]
    Spawn(#[source] std::io::Error),
    /// git fails, with its error output.
    #[error("git: {0}")]
    Failed(String),
    /// HEAD is not a branch.
    #[error("HEAD is detached: no current git branch")]
    Detached,
    /// The branch name is not UTF-8.
    #[error("git: the branch name is not UTF-8")]
    NotUtf8,
    /// The branch name is not a valid branch name.
    #[error(transparent)]
    Invalid(#[from] InvalidBranch),
}

/// The current branch of the git repository of the working directory.
fn git_branch() -> Result<BranchName, GitError> {
    let output = Command::new("git")
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .map_err(GitError::Spawn)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(match stderr.trim() {
            "" => GitError::Detached,
            error => GitError::Failed(error.to_owned()),
        });
    }
    let mut name = String::from_utf8(output.stdout).map_err(|_| GitError::NotUtf8)?;
    name.truncate(name.trim_end().len());
    Ok(BranchName::try_from(name)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `auto` is the git branch; other names are branches when they are not
    /// empty and have no white space around them.
    #[test]
    fn branches() {
        assert_eq!("auto".parse(), Ok(Branch::Auto));
        assert_eq!(
            "feature-x"
                .parse::<Branch>()
                .map(|branch| branch.to_string()),
            Ok("feature-x".to_owned())
        );
        for invalid in ["", " main", "main\n"] {
            assert!(invalid.parse::<Branch>().is_err(), "{invalid:?}");
        }
        assert_eq!(Branch::default().to_string(), DEFAULT);
    }
}
