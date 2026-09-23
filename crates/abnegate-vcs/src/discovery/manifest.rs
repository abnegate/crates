use std::fmt;

/// The manifest Composer reads a package's dependencies from.
pub(super) const COMPOSER_MANIFEST: &str = "composer.json";

/// The manifest npm reads a package's dependencies from.
pub(super) const PACKAGE_MANIFEST: &str = "package.json";

/// The package manager a dependency was declared to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Manifest {
    Composer,
    Npm,
}

impl Manifest {
    /// The package manager, by the name it goes by.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Composer => "composer",
            Self::Npm => "npm",
        }
    }

    pub(super) const fn file_name(self) -> &'static str {
        match self {
            Self::Composer => COMPOSER_MANIFEST,
            Self::Npm => PACKAGE_MANIFEST,
        }
    }
}

impl fmt::Display for Manifest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
