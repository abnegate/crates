//! Reading a checkout's package manifests for the dependencies it declares on
//! organisations the caller cares about.
//!
//! A repository that depends on another repository of the same organisation has
//! to be rebuilt when that one changes, and the manifests already say so. This
//! reads `composer.json` and `package.json` in a directory, or in every
//! directory one level below it, and keeps only the dependencies whose owner is
//! one of the organisations it was given.

use serde::Deserialize;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::Path;
use thiserror::Error;

/// A manifest file this reads dependencies out of.
const COMPOSER_MANIFEST: &str = "composer.json";
const PACKAGE_MANIFEST: &str = "package.json";

/// What went wrong reading a manifest.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Malformed manifest: {0}")]
    Malformed(#[from] serde_json::Error),
}

pub type DiscoveryResult<T> = Result<T, DiscoveryError>;

/// The package manager a dependency was declared to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Manifest {
    Composer,
    Npm,
}

impl Manifest {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Composer => "composer",
            Self::Npm => "npm",
        }
    }

    const fn file_name(self) -> &'static str {
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

/// One dependency one repository declares on another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredDependency {
    /// The repository that has the dependency.
    pub repository: String,
    /// The dependency's package name, as `organisation/package`.
    pub depends_on: String,
    /// The package manager it was declared to.
    pub manifest: Manifest,
    /// Where on disk the repository declaring it was found.
    pub repository_path: String,
}

/// Scans directories for dependencies on known organisations.
#[derive(Debug, Clone, Default)]
pub struct DependencyDiscovery {
    organizations: HashSet<String>,
}

impl DependencyDiscovery {
    /// Look only for dependencies owned by one of `organizations`.
    pub fn new(organizations: Vec<String>) -> Self {
        Self {
            organizations: organizations.into_iter().collect(),
        }
    }

    /// Whether a package name belongs to one of the known organisations.
    fn is_known(&self, package: &str) -> bool {
        match package.split('/').next() {
            Some(organization) => self.organizations.contains(organization),
            None => false,
        }
    }

    /// Read both manifests in one directory.
    ///
    /// A manifest that cannot be read or parsed contributes nothing rather than
    /// failing the scan: a directory full of repositories is scanned for what it
    /// can say, and one broken `composer.json` in it is not the caller's to fix.
    pub fn scan_directory(&self, path: &Path) -> DiscoveryResult<Vec<DiscoveredDependency>> {
        let Some(name) = self.repository_name(path) else {
            return Ok(Vec::new());
        };

        let mut dependencies: Vec<DiscoveredDependency> = Vec::new();
        for manifest in [Manifest::Composer, Manifest::Npm] {
            let file = path.join(manifest.file_name());
            if file.exists()
                && let Ok(found) = self.scan_manifest(&file, manifest, &name)
            {
                dependencies.extend(found);
            }
        }

        Ok(dependencies)
    }

    /// Read every directory in `paths`, and the directories one level below any
    /// that declares nothing itself.
    pub fn scan_directories(&self, paths: &[String]) -> DiscoveryResult<Vec<DiscoveredDependency>> {
        let mut all: Vec<DiscoveredDependency> = Vec::new();

        for path in paths.iter().map(Path::new).filter(|path| path.is_dir()) {
            if let Ok(dependencies) = self.scan_directory(path)
                && !dependencies.is_empty()
            {
                all.extend(dependencies);
                continue;
            }

            let Ok(entries) = fs::read_dir(path) else {
                continue;
            };
            for entry in entries.flatten() {
                let entry = entry.path();
                if entry.is_dir()
                    && let Ok(dependencies) = self.scan_directory(&entry)
                {
                    all.extend(dependencies);
                }
            }
        }

        Ok(all)
    }

    /// What the repository in `path` calls itself, preferring what a manifest
    /// says over what the directory is called.
    fn repository_name(&self, path: &Path) -> Option<String> {
        let composer = path.join(COMPOSER_MANIFEST);
        if composer.exists()
            && let Ok(content) = fs::read_to_string(&composer)
            && let Ok(manifest) = serde_json::from_str::<ComposerJson>(&content)
            && let Some(name) = manifest.name
        {
            return Some(name);
        }

        let package = path.join(PACKAGE_MANIFEST);
        if package.exists()
            && let Ok(content) = fs::read_to_string(&package)
            && let Ok(manifest) = serde_json::from_str::<PackageJson>(&content)
            && let Some(name) = manifest.name
        {
            return Some(unscoped(&name));
        }

        path.file_name()
            .and_then(std::ffi::OsStr::to_str)
            .map(str::to_string)
    }

    fn scan_manifest(
        &self,
        file: &Path,
        manifest: Manifest,
        repository: &str,
    ) -> DiscoveryResult<Vec<DiscoveredDependency>> {
        let content = fs::read_to_string(file)?;
        let declared: Vec<Requirements> = match manifest {
            Manifest::Composer => {
                let parsed: ComposerJson = serde_json::from_str(&content)?;
                vec![parsed.require, parsed.require_dev]
            }
            Manifest::Npm => {
                let parsed: PackageJson = serde_json::from_str(&content)?;
                vec![parsed.dependencies, parsed.dev_dependencies]
            }
        };

        let repository_path = file
            .parent()
            .map(|parent| parent.to_string_lossy().to_string())
            .unwrap_or_default();

        Ok(declared
            .into_iter()
            .flatten()
            .flat_map(HashMap::into_keys)
            .map(|package| unscoped(&package))
            .filter(|package| self.is_known(package))
            .map(|depends_on| DiscoveredDependency {
                repository: repository.to_string(),
                depends_on,
                manifest,
                repository_path: repository_path.clone(),
            })
            .collect())
    }
}

/// An npm package is scoped as `@organisation/package`; every other manifest
/// names the same pair without the marker.
fn unscoped(package: &str) -> String {
    package.trim_start_matches('@').to_string()
}

type Requirements = Option<HashMap<String, serde_json::Value>>;

#[derive(Deserialize)]
struct ComposerJson {
    name: Option<String>,
    require: Requirements,
    #[serde(rename = "require-dev")]
    require_dev: Requirements,
}

#[derive(Deserialize)]
struct PackageJson {
    name: Option<String>,
    dependencies: Requirements,
    #[serde(rename = "devDependencies")]
    dev_dependencies: Requirements,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn discovery(organizations: &[&str]) -> DependencyDiscovery {
        DependencyDiscovery::new(organizations.iter().map(|name| name.to_string()).collect())
    }

    fn write(directory: &Path, name: &str, manifest: serde_json::Value) {
        std::fs::write(directory.join(name), manifest.to_string()).unwrap();
    }

    #[test]
    fn a_package_belongs_to_an_organisation_this_was_told_about() {
        let discovery = discovery(&["utopia-php", "appwrite"]);

        assert!(discovery.is_known("utopia-php/database"));
        assert!(discovery.is_known("appwrite/sdk"));
        assert!(
            discovery.is_known("utopia-php"),
            "a name with no owner is its own owner"
        );
        assert!(!discovery.is_known("symfony/console"));
        assert!(!discovery.is_known("laravel/framework"));
        assert!(!discovery.is_known(""));
    }

    #[test]
    fn an_organisation_is_matched_exactly() {
        let discovery = discovery(&["Appwrite"]);

        assert!(discovery.is_known("Appwrite/sdk"));
        assert!(!discovery.is_known("appwrite/sdk"));
        assert!(!self::discovery(&[]).is_known("utopia-php/database"));
    }

    #[test]
    fn an_npm_scope_marker_is_not_part_of_the_organisation() {
        assert_eq!(unscoped("@appwrite/sdk"), "appwrite/sdk");
        assert_eq!(unscoped("react"), "react");
    }

    #[test]
    fn a_directory_with_no_manifests_declares_nothing() {
        let directory = TempDir::new().unwrap();

        assert!(
            discovery(&["utopia-php"])
                .scan_directory(directory.path())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_composer_manifest_yields_its_requirements_from_both_sections() {
        let directory = TempDir::new().unwrap();
        write(
            directory.path(),
            COMPOSER_MANIFEST,
            serde_json::json!({
                "name": "appwrite/cloud",
                "require": { "utopia-php/database": "^1.0", "symfony/console": "^5.0" },
                "require-dev": { "utopia-php/testing": "^1.0" },
            }),
        );

        let mut dependencies = discovery(&["utopia-php"])
            .scan_directory(directory.path())
            .unwrap();
        dependencies.sort_by(|left, right| left.depends_on.cmp(&right.depends_on));

        assert_eq!(dependencies.len(), 2);
        assert_eq!(dependencies[0].depends_on, "utopia-php/database");
        assert_eq!(dependencies[1].depends_on, "utopia-php/testing");
        assert_eq!(dependencies[0].repository, "appwrite/cloud");
        assert_eq!(dependencies[0].manifest, Manifest::Composer);
        assert_eq!(dependencies[0].manifest.to_string(), "composer");
        assert_eq!(
            dependencies[0].repository_path,
            directory.path().to_string_lossy()
        );
    }

    #[test]
    fn a_package_manifest_yields_its_dependencies_from_both_sections() {
        let directory = TempDir::new().unwrap();
        write(
            directory.path(),
            PACKAGE_MANIFEST,
            serde_json::json!({
                "name": "@appwrite/console",
                "dependencies": { "@appwrite/sdk": "^1.0", "react": "^18.0" },
                "devDependencies": { "@appwrite/testing": "^1.0" },
            }),
        );

        let mut dependencies = discovery(&["appwrite"])
            .scan_directory(directory.path())
            .unwrap();
        dependencies.sort_by(|left, right| left.depends_on.cmp(&right.depends_on));

        assert_eq!(dependencies.len(), 2);
        assert_eq!(dependencies[0].depends_on, "appwrite/sdk");
        assert_eq!(dependencies[1].depends_on, "appwrite/testing");
        assert_eq!(dependencies[0].manifest, Manifest::Npm);
        assert_eq!(dependencies[0].manifest.as_str(), "npm");
    }

    #[test]
    fn a_manifest_this_cannot_read_leaves_the_scan_saying_nothing() {
        let directory = TempDir::new().unwrap();
        std::fs::write(directory.path().join(COMPOSER_MANIFEST), "{ invalid json }").unwrap();

        assert!(
            discovery(&["utopia-php"])
                .scan_directory(directory.path())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_manifest_that_names_nothing_leaves_the_directory_to_name_the_repository() {
        let directory = TempDir::new().unwrap();
        write(
            directory.path(),
            COMPOSER_MANIFEST,
            serde_json::json!({ "require": { "utopia-php/database": "^1.0" } }),
        );

        let dependencies = discovery(&["utopia-php"])
            .scan_directory(directory.path())
            .unwrap();

        assert_eq!(dependencies.len(), 1);
        assert_eq!(
            dependencies[0].repository,
            directory.path().file_name().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn a_manifest_that_requires_nothing_declares_nothing() {
        let directory = TempDir::new().unwrap();
        write(
            directory.path(),
            COMPOSER_MANIFEST,
            serde_json::json!({ "name": "myapp", "require": {} }),
        );

        assert!(
            discovery(&["utopia-php"])
                .scan_directory(directory.path())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_composer_name_is_preferred_and_an_npm_scope_is_stripped_from_one() {
        let both = TempDir::new().unwrap();
        write(
            both.path(),
            COMPOSER_MANIFEST,
            serde_json::json!({ "name": "composer-name" }),
        );
        write(
            both.path(),
            PACKAGE_MANIFEST,
            serde_json::json!({ "name": "package-name" }),
        );
        assert_eq!(
            discovery(&[]).repository_name(both.path()),
            Some("composer-name".to_string())
        );

        let scoped = TempDir::new().unwrap();
        write(
            scoped.path(),
            PACKAGE_MANIFEST,
            serde_json::json!({ "name": "@org/package" }),
        );
        assert_eq!(
            discovery(&[]).repository_name(scoped.path()),
            Some("org/package".to_string())
        );

        let bare = TempDir::new().unwrap();
        assert_eq!(
            discovery(&[]).repository_name(bare.path()),
            bare.path()
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
            "a directory with no manifest is named after itself"
        );
    }

    #[test]
    fn a_path_that_declares_nothing_itself_is_read_one_level_down() {
        let root = TempDir::new().unwrap();
        let project = root.path().join("my-project");
        std::fs::create_dir(&project).unwrap();
        write(
            &project,
            COMPOSER_MANIFEST,
            serde_json::json!({
                "name": "my-project",
                "require": { "utopia-php/database": "^1.0" },
            }),
        );

        let dependencies = discovery(&["utopia-php"])
            .scan_directories(&[root.path().to_string_lossy().to_string()])
            .unwrap();

        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].depends_on, "utopia-php/database");
        assert_eq!(dependencies[0].repository, "my-project");
    }

    #[test]
    fn a_path_that_declares_something_itself_is_not_read_one_level_down() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            COMPOSER_MANIFEST,
            serde_json::json!({
                "name": "outer",
                "require": { "utopia-php/database": "^1.0" },
            }),
        );
        let inner = root.path().join("inner");
        std::fs::create_dir(&inner).unwrap();
        write(
            &inner,
            COMPOSER_MANIFEST,
            serde_json::json!({
                "name": "inner",
                "require": { "utopia-php/cache": "^1.0" },
            }),
        );

        let dependencies = discovery(&["utopia-php"])
            .scan_directories(&[root.path().to_string_lossy().to_string()])
            .unwrap();

        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].repository, "outer");
    }

    #[test]
    fn a_path_that_is_not_there_is_nothing_to_scan() {
        let discovery = discovery(&["utopia-php"]);

        assert!(discovery.scan_directories(&[]).unwrap().is_empty());
        assert!(
            discovery
                .scan_directories(&["/nonexistent/path".to_string()])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_discovered_dependency_carries_everything_needed_to_act_on_it() {
        let dependency = DiscoveredDependency {
            repository: "my-app".to_string(),
            depends_on: "utopia-php/database".to_string(),
            manifest: Manifest::Composer,
            repository_path: "/path/to/app".to_string(),
        };

        assert_eq!(dependency, dependency.clone());
        assert_eq!(dependency.repository, "my-app");
        assert_eq!(dependency.depends_on, "utopia-php/database");
        assert_eq!(dependency.manifest.as_str(), "composer");
        assert_eq!(dependency.repository_path, "/path/to/app");
    }
}
