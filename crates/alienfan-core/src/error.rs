//! Error types. Messages are pt-BR because they reach the UI unchanged.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `EACCES`/`EPERM` on a sysfs node or on the config.
    #[error("sem permissão para escrever em {}", path.display())]
    NoPermission { path: PathBuf },

    /// Driver not loaded, node gone (`ENOENT`/`ENODEV`) or device not found.
    #[error("hardware não encontrado: {0}")]
    NoDriver(String),

    /// A value failed validation (also `EINVAL` from the kernel).
    #[error("{0}")]
    Invalid(String),

    /// `DeleteCurve` on a curve that a default still uses.
    #[error("a curva \"{name}\" está em uso no padrão {used_by}")]
    CurveInUse { name: String, used_by: String },

    #[error("config inválida: {0}")]
    Config(#[from] ConfigError),

    #[error("erro de E/S em {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl Error {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    /// Classifies an I/O error on `path` as SPEC 6.5 asks.
    pub(crate) fn from_io(path: &Path, source: io::Error) -> Self {
        match source.raw_os_error() {
            Some(libc::EACCES | libc::EPERM) => Self::NoPermission {
                path: path.to_owned(),
            },
            Some(libc::ENOENT | libc::ENODEV | libc::ENXIO) => {
                Self::NoDriver(format!("{} sumiu", path.display()))
            }
            Some(libc::EINVAL) => Self::Invalid(format!(
                "o kernel recusou o valor escrito em {}",
                path.display()
            )),
            _ => Self::Io {
                path: path.to_owned(),
                source,
            },
        }
    }
}

/// A config that failed to parse or validate. `line`/`column` are 1-based.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct ConfigError {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl ConfigError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            line: None,
            column: None,
        }
    }

    /// Builds the error with the position of byte `offset` inside `source`.
    pub(crate) fn at(message: impl Into<String>, source: &str, offset: usize) -> Self {
        let before = &source[..offset.min(source.len())];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        Self {
            message: message.into(),
            line: Some(line),
            column: Some(column),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.line, self.column) {
            (Some(line), Some(column)) => {
                write!(f, "linha {line}, coluna {column}: {}", self.message)
            }
            _ => f.write_str(&self.message),
        }
    }
}
