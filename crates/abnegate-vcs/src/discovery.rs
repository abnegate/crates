//! Reading a checkout's package manifests for the dependencies it declares on
//! organisations the caller cares about.
//!
//! A repository that depends on another repository of the same organisation has
//! to be rebuilt when that one changes, and the manifests already say so. This
//! reads `composer.json` and `package.json` in a directory, or in every
//! directory one level below it, and keeps only the dependencies whose owner is
//! one of the organisations it was given.

mod composer_manifest;
mod discovered_dependency;
mod manifest;
mod package_manifest;
mod requirements;

use crate::discovery::composer_manifest::ComposerManifest;
pub use crate::discovery::discovered_dependency::DiscoveredDependency;
pub use crate::discovery::manifest::Manifest;
use crate::discovery::package_manifest::PackageManifest;
use serde::de::DeserializeOwned;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

/// The marker npm puts ahead of a scoped package's organisation.
const SCOPE: char = '@';

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
        let organization = package.split_once('/').map_or(package, |(owner, _)| owner);
        self.organizations.contains(organization)
    }

    /// Read both manifests in one directory.
    ///
    /// A manifest that cannot be read or parsed contributes nothing rather than
    /// failing the scan, and is reported as skipped: a directory full of
    /// repositories is scanned for what it can say, and one broken
    /// `composer.json` in it is not the caller's to fix. A manifest that is a
    /// link rather than a file is skipped the same way, so a scan reads only
    /// what the directory itself holds.
    pub fn scan_directory(&self, path: &Path) -> Vec<DiscoveredDependency> {
        let composer: Option<ComposerManifest> = read(path, Manifest::Composer);
        let package: Option<PackageManifest> = read(path, Manifest::Npm);

        let name = composer
            .as_ref()
            .and_then(|manifest| manifest.name.clone())
            .or_else(|| {
                package
                    .as_ref()
                    .and_then(|manifest| manifest.name.as_deref().map(unscoped))
            })
            .or_else(|| {
                path.file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .map(str::to_string)
            });
        let Some(name) = name else {
            return Vec::new();
        };

        let mut dependencies = Vec::new();
        if let Some(composer) = &composer {
            dependencies.extend(self.declared(
                composer.require.names().chain(composer.require_dev.names()),
                Manifest::Composer,
                &name,
                path,
            ));
        }
        if let Some(package) = &package {
            dependencies.extend(
                self.declared(
                    package
                        .dependencies
                        .names()
                        .chain(package.dev_dependencies.names()),
                    Manifest::Npm,
                    &name,
                    path,
                ),
            );
        }
        dependencies
    }

    /// Read every directory in `paths`, and the directories one level below any
    /// that declares nothing itself.
    pub fn scan_directories(&self, paths: &[PathBuf]) -> Vec<DiscoveredDependency> {
        let mut all: Vec<DiscoveredDependency> = Vec::new();

        for path in paths.iter().filter(|path| path.is_dir()) {
            let dependencies = self.scan_directory(path);
            if !dependencies.is_empty() {
                all.extend(dependencies);
                continue;
            }

            let Ok(entries) = fs::read_dir(path) else {
                continue;
            };
            for entry in entries.flatten() {
                let entry = entry.path();
                if entry.is_dir() {
                    all.extend(self.scan_directory(&entry));
                }
            }
        }

        all
    }

    /// The known organisations' packages among `packages`, each once however
    /// many sections of the manifest name it.
    fn declared<'a>(
        &self,
        packages: impl Iterator<Item = &'a str>,
        manifest: Manifest,
        repository: &str,
        path: &Path,
    ) -> Vec<DiscoveredDependency> {
        packages
            .map(unscoped)
            .filter(|package| self.is_known(package))
            .collect::<BTreeSet<String>>()
            .into_iter()
            .map(|depends_on| DiscoveredDependency {
                repository: repository.to_string(),
                depends_on,
                manifest,
                repository_path: path.to_path_buf(),
            })
            .collect()
    }
}

/// The manifest of `kind` in `directory`, if there is one that is a regular
/// file and parses.
fn read<T: DeserializeOwned>(directory: &Path, kind: Manifest) -> Option<T> {
    let file = directory.join(kind.file_name());
    let details = fs::symlink_metadata(&file).ok()?;
    if !details.is_file() {
        tracing::warn!(manifest = ?file, "Skipped a manifest that is not a regular file");
        return None;
    }
    let parsed = fs::read_to_string(&file)
        .map_err(|error| error.to_string())
        .and_then(|content| serde_json::from_str(&content).map_err(|error| error.to_string()));
    match parsed {
        Ok(manifest) => Some(manifest),
        Err(error) => {
            tracing::warn!(manifest = ?file, %error, "Skipped a manifest that could not be read");
            None
        }
    }
}

/// An npm package is scoped as `@organisation/package`; every other manifest
/// names the same pair without the marker.
fn unscoped(package: &str) -> String {
    package.trim_start_matches(SCOPE).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::manifest::COMPOSER_MANIFEST;
    use crate::discovery::manifest::PACKAGE_MANIFEST;
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

        let mut dependencies = discovery(&["utopia-php"]).scan_directory(directory.path());
        dependencies.sort_by(|left, right| left.depends_on.cmp(&right.depends_on));

        assert_eq!(dependencies.len(), 2);
        assert_eq!(dependencies[0].depends_on, "utopia-php/database");
        assert_eq!(dependencies[1].depends_on, "utopia-php/testing");
        assert_eq!(dependencies[0].repository, "appwrite/cloud");
        assert_eq!(dependencies[0].manifest, Manifest::Composer);
        assert_eq!(dependencies[0].manifest.to_string(), "composer");
        assert_eq!(dependencies[0].repository_path, directory.path());
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

        let mut dependencies = discovery(&["appwrite"]).scan_directory(directory.path());
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

        let dependencies = discovery(&["utopia-php"]).scan_directory(directory.path());

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
                .is_empty()
        );
    }

    #[test]
    fn a_composer_name_is_preferred_and_an_npm_scope_is_stripped_from_one() {
        let depending = serde_json::json!({ "utopia-php/database": "^1.0" });
        let named = |directory: &Path| {
            discovery(&["utopia-php"])
                .scan_directory(directory)
                .first()
                .map(|dependency| dependency.repository.clone())
        };

        let both = TempDir::new().unwrap();
        write(
            both.path(),
            COMPOSER_MANIFEST,
            serde_json::json!({ "name": "composer-name", "require": depending }),
        );
        write(
            both.path(),
            PACKAGE_MANIFEST,
            serde_json::json!({ "name": "package-name" }),
        );
        assert_eq!(named(both.path()), Some("composer-name".to_string()));

        let scoped = TempDir::new().unwrap();
        write(
            scoped.path(),
            PACKAGE_MANIFEST,
            serde_json::json!({ "name": "@org/package", "dependencies": { "@utopia-php/x": "1" } }),
        );
        assert_eq!(named(scoped.path()), Some("org/package".to_string()));
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

        let dependencies =
            discovery(&["utopia-php"]).scan_directories(&[root.path().to_path_buf()]);

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

        let dependencies =
            discovery(&["utopia-php"]).scan_directories(&[root.path().to_path_buf()]);

        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].repository, "outer");
    }

    #[test]
    fn a_path_that_is_not_there_is_nothing_to_scan() {
        let discovery = discovery(&["utopia-php"]);

        assert!(discovery.scan_directories(&[]).is_empty());
        assert!(
            discovery
                .scan_directories(&[PathBuf::from("/nonexistent/path")])
                .is_empty()
        );
    }

    #[test]
    fn a_discovered_dependency_carries_everything_needed_to_act_on_it() {
        let dependency = DiscoveredDependency {
            repository: "my-app".to_string(),
            depends_on: "utopia-php/database".to_string(),
            manifest: Manifest::Composer,
            repository_path: PathBuf::from("/path/to/app"),
        };

        assert_eq!(dependency, dependency.clone());
        assert_eq!(dependency.repository, "my-app");
        assert_eq!(dependency.depends_on, "utopia-php/database");
        assert_eq!(dependency.manifest.as_str(), "composer");
        assert_eq!(dependency.repository_path, Path::new("/path/to/app"));
    }

    /// PHP encodes an empty object as `[]`, and one empty section used to
    /// fail the whole manifest, losing every other section with it.
    #[test]
    fn an_empty_section_written_as_a_list_requires_nothing_and_loses_nothing() {
        let directory = TempDir::new().unwrap();
        write(
            directory.path(),
            COMPOSER_MANIFEST,
            serde_json::json!({
                "name": "appwrite/cloud",
                "require": [],
                "require-dev": { "utopia-php/cli": "^1.0" },
            }),
        );

        let dependencies = discovery(&["utopia-php"]).scan_directory(directory.path());

        assert_eq!(dependencies.len(), 1, "{dependencies:?}");
        assert_eq!(dependencies[0].depends_on, "utopia-php/cli");
    }

    #[test]
    fn a_package_both_sections_require_is_one_dependency() {
        let directory = TempDir::new().unwrap();
        write(
            directory.path(),
            PACKAGE_MANIFEST,
            serde_json::json!({
                "name": "console",
                "dependencies": { "@appwrite/sdk": "^1.0", "appwrite/sdk": "^1.0" },
                "devDependencies": { "@appwrite/sdk": "^1.0" },
            }),
        );

        let dependencies = discovery(&["appwrite"]).scan_directory(directory.path());

        assert_eq!(dependencies.len(), 1, "{dependencies:?}");
        assert_eq!(dependencies[0].depends_on, "appwrite/sdk");
    }

    #[cfg(unix)]
    #[test]
    fn a_manifest_that_is_a_link_is_not_read() {
        let outside = TempDir::new().unwrap();
        write(
            outside.path(),
            COMPOSER_MANIFEST,
            serde_json::json!({ "name": "elsewhere", "require": { "utopia-php/database": "1" } }),
        );
        let directory = TempDir::new().unwrap();
        std::os::unix::fs::symlink(
            outside.path().join(COMPOSER_MANIFEST),
            directory.path().join(COMPOSER_MANIFEST),
        )
        .unwrap();

        assert!(
            discovery(&["utopia-php"])
                .scan_directory(directory.path())
                .is_empty()
        );
    }
}
