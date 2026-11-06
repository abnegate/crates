//! The agent's own conversation a run starts or carries on, and the file the
//! agent keeps it in.
//!
//! Claude keeps a session at `projects/<project>/<id>.jsonl` in its
//! configuration directory, and Codex at
//! `sessions/<yyyy>/<mm>/<dd>/rollout-<stamp>-<id>.jsonl` in its home. Both
//! resume a session by reading that file, so a copy under the same relative
//! path is all another home needs. Claude finds a session by the directory it
//! runs in, so a session whose run now starts somewhere else is moved to that
//! directory's folder within its home.

mod carried;
mod session_error;

use std::fs;
use std::fs::DirBuilder;
use std::fs::FileType;
use std::fs::OpenOptions;
use std::fs::Permissions;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

use nix::fcntl::OFlag;
use tempfile::Builder;

use crate::kind::AgentKind;

pub use crate::session::carried::Carried;
pub use crate::session::session_error::SessionError;

const PROJECTS: &str = "projects";
const SESSIONS: &str = "sessions";
const ROLLOUT: &str = "rollout-";
const EXTENSION: &str = ".jsonl";
const STAGING: &str = ".carrying-";
/// A copy of a Claude session, relative to its home, and when it was written.
type Found = (PathBuf, SystemTime);

/// How many folders, a year's, a month's and a day's, a rollout sits in
/// within Codex's sessions folder.
const DATE_DEPTH: usize = 3;

/// Claude cuts a longer project folder name to this many UTF-16 units and
/// appends a hash of the whole path.
const FOLDER_LIMIT: usize = 200;
const SEPARATOR: char = '-';
const HASH_RADIX: u32 = 36;

const FILE_MODE: u32 = 0o600;
const DIRECTORY_MODE: u32 = 0o700;

/// The agent's own session a run starts under an id of the caller's
/// choosing, or resumes, set with
/// [`CliSettings::with_session`](crate::CliSettings::with_session).
///
/// Claude starts a [pinned](Session::pinned) session under the id given, with
/// `--session-id`, and resumes one with `--resume`. Codex names its own
/// threads, so a pinned session leaves its run as it would be without one,
/// and only a [resumed](Session::resumed) one reaches it, as
/// `codex exec resume <id>`. Either agent reports the session it ran in as
/// [`AgentEvent::Session`](crate::AgentEvent::Session). Claude takes only a
/// UUID as a pinned session's id.
///
/// A resumed session already holds the turns it ran, and the run sends the
/// agent every message in the request it is given. Which of a conversation's
/// messages the session has seen is the host's to track, so a host resuming
/// one sends only the messages after them, which
/// [`transcript::render_tail`](crate::transcript::render_tail) renders.
///
/// A session lives in a file in the agent's home: Claude's configuration
/// directory (`CLAUDE_CONFIG_DIR`, or `~/.claude`) or Codex's `CODEX_HOME`
/// (or `~/.codex`). [`Session::locate`] finds that file,
/// [`Session::carry`] copies it into another home so a run signed in there
/// resumes it, and [`Session::relocate`] moves a Claude session to the folder
/// of the directory its next run starts in. None of them follows a link
/// between the home and the file, and an id other than ASCII letters, digits
/// and `-` names no session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Session {
    /// The session's id, as the agent spells it.
    pub id: String,
    /// Whether the session already exists and the run carries it on, rather
    /// than starting it.
    pub resume: bool,
}

impl Session {
    /// A new session the run starts under `id`.
    pub fn pinned(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            resume: false,
        }
    }

    /// The existing session `id`, which the run carries on.
    pub fn resumed(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            resume: true,
        }
    }

    /// The file `agent` keeps this session in under `home`, for runs in
    /// `work`: `None` when there is none, the id names no session, or the
    /// way to it from `home` passes through a link.
    pub fn locate(&self, agent: AgentKind, home: &Path, work: &Path) -> Option<PathBuf> {
        relative(home, agent, work, &self.id).map(|relative| home.join(relative))
    }

    /// Copies this session's file from the home `from` into the home `to`,
    /// under the path `agent` looks for it there when it runs in `work`,
    /// readable only by its owner. The newest copy in `from` is the one
    /// carried, wherever it last ran. A file already at the path is replaced
    /// whole, never partly, and each directory missing on the way to it is
    /// created readable only by its owner; a directory there that is a link
    /// is refused, so nothing is written outside `to`.
    pub fn carry(
        &self,
        agent: AgentKind,
        from: &Path,
        to: &Path,
        work: &Path,
    ) -> Result<Carried, SessionError> {
        let found = self.stored(agent, from)?;
        let relative = match agent {
            AgentKind::Claude => claude(work, &self.id),
            AgentKind::Codex => found.clone(),
        };
        let source = from.join(&found);
        let path = to.join(&relative);
        let parent = path.parent().unwrap_or(to);

        directories(to, relative.parent().unwrap_or(Path::new(""))).map_err(|source| {
            SessionError::Filesystem {
                path: parent.to_path_buf(),
                source,
            }
        })?;

        let bytes = copy(&source, &path).map_err(|source| SessionError::Filesystem {
            path: path.clone(),
            source,
        })?;
        Ok(Carried::new(path, bytes))
    }

    /// Moves this session within `home` to where `agent` looks for it when it
    /// runs in `work`. Claude looks in a folder named for the directory it
    /// runs in, and a session stays in its folder when a run starts somewhere
    /// new, so a conversation that changes directory would otherwise lose its
    /// session. Codex finds a session by its id alone, so its sessions are
    /// left where they are.
    ///
    /// Carrying a session leaves the copy it came from, so a home can hold an
    /// old one, even in the folder `work` names. The newest is the session,
    /// and the rest are removed: resuming an old one would lose every turn
    /// since without anything failing. The folder Claude keeps beside a
    /// transcript stays where it is, since the transcript cites what is in it
    /// by absolute path.
    pub fn relocate(&self, agent: AgentKind, home: &Path, work: &Path) -> Result<(), SessionError> {
        if agent == AgentKind::Codex {
            return Ok(());
        }
        if !named(&self.id) {
            return Err(SessionError::Invalid {
                id: self.id.clone(),
            });
        }
        let projects = home.join(PROJECTS);
        let copies = every_copy(home, &self.id).map_err(|source| SessionError::Filesystem {
            path: projects,
            source,
        })?;
        let found = copies
            .iter()
            .max_by_key(|(_, modified)| *modified)
            .map(|(relative, _)| relative.clone())
            .ok_or_else(|| SessionError::Missing {
                agent,
                id: self.id.clone(),
            })?;
        let relative = claude(work, &self.id);
        let path = home.join(&relative);
        let source = home.join(&found);
        if !same_file(&source, &path) {
            directories(home, relative.parent().unwrap_or(Path::new("")))
                .and_then(|()| fs::rename(&source, &path))
                .map_err(|source| SessionError::Filesystem {
                    path: path.clone(),
                    source,
                })?;
        }
        for (copy, _) in copies.into_iter().filter(|(copy, _)| *copy != found) {
            let copy = home.join(copy);
            if !same_file(&copy, &path) {
                fs::remove_file(&copy)
                    .map_err(|source| SessionError::Filesystem { path: copy, source })?;
            }
        }
        Ok(())
    }

    /// The name of the folder Claude keeps the sessions of runs in `work`
    /// under, within `projects` in its configuration directory: `work` with
    /// every link resolved, as the CLI's `getcwd` reports it, and every UTF-16
    /// unit that is not an ASCII letter or digit made `-`. A name longer than
    /// 200 units is cut there and followed by `-` and the base-36 magnitude of
    /// the path's Java-style string hash.
    pub fn project(work: &Path) -> String {
        sanitized(&physical(work))
    }

    fn stored(&self, agent: AgentKind, home: &Path) -> Result<PathBuf, SessionError> {
        if !named(&self.id) {
            return Err(SessionError::Invalid {
                id: self.id.clone(),
            });
        }
        newest(home, agent, &self.id).ok_or_else(|| SessionError::Missing {
            agent,
            id: self.id.clone(),
        })
    }
}

/// Whether two paths are one file, as two spellings of a path are on a
/// filesystem that ignores case.
fn same_file(one: &Path, other: &Path) -> bool {
    match (fs::symlink_metadata(one), fs::symlink_metadata(other)) {
        (Ok(one), Ok(other)) => one.dev() == other.dev() && one.ino() == other.ino(),
        _ => false,
    }
}

fn sanitized(work: &Path) -> String {
    let units: Vec<u16> = work.to_string_lossy().encode_utf16().collect();
    let name: String = units
        .iter()
        .map(|&unit| match char::from_u32(u32::from(unit)) {
            Some(character) if character.is_ascii_alphanumeric() => character,
            _ => SEPARATOR,
        })
        .collect();

    if units.len() <= FOLDER_LIMIT {
        return name;
    }

    let hash = units.iter().fold(0_i32, |hash, &unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    });
    format!(
        "{}{SEPARATOR}{}",
        &name[..FOLDER_LIMIT],
        base36(hash.unsigned_abs())
    )
}

fn relative(home: &Path, agent: AgentKind, work: &Path, id: &str) -> Option<PathBuf> {
    if !named(id) {
        return None;
    }

    let relative = match agent {
        AgentKind::Claude => {
            let relative = claude(work, id);
            regular(&home.join(&relative)).then_some(relative)
        }
        AgentKind::Codex => rollout(home, Path::new(SESSIONS), &format!("-{id}{EXTENSION}")),
    }?;
    relative
        .parent()
        .is_none_or(|parent| unlinked(home, parent))
        .then_some(relative)
}

/// Where Claude keeps session `id` for runs in `work`, relative to its home.
fn claude(work: &Path, id: &str) -> PathBuf {
    Path::new(PROJECTS)
        .join(Session::project(work))
        .join(format!("{id}{EXTENSION}"))
}

/// Session `id` wherever in `home` `agent` left it, whichever directory it
/// last ran in: the most recently written copy, when carrying it left more
/// than one.
fn newest(home: &Path, agent: AgentKind, id: &str) -> Option<PathBuf> {
    match agent {
        AgentKind::Claude => copies(home, id)
            .into_iter()
            .max_by_key(|(_, modified)| *modified)
            .map(|(relative, _)| relative),
        AgentKind::Codex => rollout(home, Path::new(SESSIONS), &format!("-{id}{EXTENSION}"))
            .filter(|relative| {
                relative
                    .parent()
                    .is_none_or(|parent| unlinked(home, parent))
            }),
    }
}

/// Every copy of Claude's session `id` under `home`, relative to it, and when
/// each was written, leaving out whatever cannot be read.
fn copies(home: &Path, id: &str) -> Vec<Found> {
    readings(home, id)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|reading| reading.ok().flatten())
        .collect()
}

/// Every copy of Claude's session `id` under `home`, as [`copies`] lists
/// them, failing when the folders holding them, or any one of them, cannot
/// be read, so a copy left unread is never taken for no copy.
fn every_copy(home: &Path, id: &str) -> io::Result<Vec<Found>> {
    readings(home, id)?
        .into_iter()
        .filter_map(Result::transpose)
        .collect()
}

/// What each folder in Claude's `projects` says of a copy of session `id`:
/// the copy and when it was written, or nothing when it holds none or is
/// no folder of its own, a link to one included.
fn readings(home: &Path, id: &str) -> io::Result<Vec<io::Result<Option<Found>>>> {
    if !named(id) {
        return Ok(Vec::new());
    }
    let folders = match fs::read_dir(home.join(PROJECTS)) {
        Ok(folders) => folders,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    Ok(folders
        .map(|folder| {
            let folder = folder?;
            if !folder.file_type()?.is_dir() {
                return Ok(None);
            }
            let relative = Path::new(PROJECTS)
                .join(folder.file_name())
                .join(format!("{id}{EXTENSION}"));
            let metadata = match fs::symlink_metadata(home.join(&relative)) {
                Ok(metadata) => metadata,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
            if !metadata.file_type().is_file()
                || !relative
                    .parent()
                    .is_none_or(|parent| unlinked(home, parent))
            {
                return Ok(None);
            }
            Ok(Some((relative, metadata.modified()?)))
        })
        .collect())
}

/// Whether every directory on `relative`'s way down from `home` is a
/// directory itself, not a link to one that could lie outside the home.
fn unlinked(home: &Path, relative: &Path) -> bool {
    let mut path = home.to_path_buf();
    relative.components().all(|component| {
        path.push(component);
        real(&path)
    })
}

/// Creates each directory on `relative`'s way down from `home` that is
/// missing, refusing one that is a link, so nothing is written outside the
/// home.
fn directories(home: &Path, relative: &Path) -> io::Result<()> {
    let mut path = home.to_path_buf();
    for component in relative.components() {
        path.push(component);
        match DirBuilder::new().mode(DIRECTORY_MODE).create(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        if !real(&path) {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is a link or not a directory", path.display()),
            ));
        }
    }
    Ok(())
}

fn real(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// Session ids are UUIDs. Anything else could name a path outside the session
/// folders.
fn named(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == SEPARATOR)
}

/// The directory as the CLI's `getcwd` reports it, which resolves every link
/// on the way.
fn physical(work: &Path) -> PathBuf {
    fs::canonicalize(work).unwrap_or_else(|_| work.to_path_buf())
}

fn regular(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

/// The rollout under `home/sessions` whose name ends in `suffix`, looked for
/// only where Codex keeps one, in a day's folder within a month's within a
/// year's, without following links.
fn rollout(home: &Path, sessions: &Path, suffix: &str) -> Option<PathBuf> {
    let mut folders = vec![sessions.to_path_buf()];
    for _ in 0..DATE_DEPTH {
        folders = folders
            .iter()
            .flat_map(|folder| entries(home, folder))
            .filter(|(_, kind)| kind.is_dir())
            .map(|(relative, _)| relative)
            .collect();
    }
    folders.iter().find_map(|day| {
        entries(home, day).into_iter().find_map(|(relative, kind)| {
            (kind.is_file()
                && relative
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_prefix(ROLLOUT))
                    .and_then(|rest| rest.strip_suffix(suffix))
                    .is_some_and(|stamp| !stamp.is_empty()))
            .then_some(relative)
        })
    })
}

/// What `home/directory` holds, each relative to `home` with its own type,
/// a link read as a link.
fn entries(home: &Path, directory: &Path) -> Vec<(PathBuf, FileType)> {
    let Ok(entries) = fs::read_dir(home.join(directory)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| Some((directory.join(entry.file_name()), entry.file_type().ok()?)))
        .collect()
}

/// Copies `source`, never through a link, into a private file staged beside
/// `destination` and synced to disk, then puts it in `destination`'s place
/// in one rename. The staged file is removed when anything fails. A pipe in
/// `source`'s place is opened without waiting for a writer, and refused with
/// anything else that is not a regular file once open.
fn copy(source: &Path, destination: &Path) -> io::Result<u64> {
    let mut reader = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
        .open(source)?;
    if !reader.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a regular file", source.display()),
        ));
    }
    let parent = destination.parent().unwrap_or(Path::new(""));
    let mut staged = Builder::new()
        .prefix(STAGING)
        .permissions(Permissions::from_mode(FILE_MODE))
        .tempfile_in(parent)?;
    let bytes = io::copy(&mut reader, staged.as_file_mut())?;
    staged.as_file().sync_all()?;
    staged.persist(destination).map_err(|error| error.error)?;
    Ok(bytes)
}

fn base36(mut value: u32) -> String {
    let mut digits = Vec::new();
    loop {
        digits.push(char::from_digit(value % HASH_RADIX, HASH_RADIX).unwrap_or(SEPARATOR));
        value /= HASH_RADIX;
        if value == 0 {
            break;
        }
    }
    digits.iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::fs::symlink;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Duration;
    use std::time::SystemTime;

    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;
    use tempfile::TempDir;

    use super::Carried;
    use super::EXTENSION;
    use super::FILE_MODE;
    use super::PROJECTS;
    use super::ROLLOUT;
    use super::Session;
    use super::SessionError;
    use super::copy;
    use super::sanitized;
    use crate::kind::AgentKind;

    const CLAUDE_ID: &str = "6f1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
    const CODEX_ID: &str = "019b2c41-0000-7000-8000-000000000001";
    const CODEX_DAY: &str = "sessions/2026/09/23";
    const TRANSCRIPT: &[u8] = b"{\"type\":\"user\",\"message\":\"remember marigold\"}\n";

    /// Makes `path` an hour older than anything written now.
    fn age(path: &Path) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
    }

    struct Homes {
        from: TempDir,
        to: TempDir,
        work: TempDir,
    }

    impl Homes {
        fn new() -> Self {
            Self {
                from: TempDir::new().unwrap(),
                to: TempDir::new().unwrap(),
                work: TempDir::new().unwrap(),
            }
        }

        fn claude(&self) -> PathBuf {
            folder(self.work.path()).join(format!("{CLAUDE_ID}{EXTENSION}"))
        }

        fn codex(&self) -> PathBuf {
            Path::new(CODEX_DAY).join(format!(
                "{ROLLOUT}2026-09-23T04-00-00-{CODEX_ID}{EXTENSION}"
            ))
        }

        fn write(&self, relative: &Path) {
            let path = self.from.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, TRANSCRIPT).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }

        fn carry(&self, agent: AgentKind, id: &str) -> Result<Carried, SessionError> {
            Session::resumed(id).carry(agent, self.from.path(), self.to.path(), self.work.path())
        }

        fn locate(&self, home: &Path, agent: AgentKind, work: &Path, id: &str) -> Option<PathBuf> {
            Session::resumed(id).locate(agent, home, work)
        }

        fn relocate(&self, agent: AgentKind, work: &Path, id: &str) -> Result<(), SessionError> {
            Session::resumed(id).relocate(agent, self.from.path(), work)
        }
    }

    /// Claude's folder for runs in `work`, relative to its home.
    fn folder(work: &Path) -> PathBuf {
        Path::new(PROJECTS).join(sanitized(&fs::canonicalize(work).unwrap()))
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn empty(path: &Path) -> bool {
        fs::read_dir(path).unwrap().next().is_none()
    }

    #[test]
    fn a_pinned_session_starts_and_a_resumed_one_carries_on() {
        let pinned = Session::pinned("5b0c1f7e-8d43-4a77-9a3e-2f0d6c1b9e42");
        let resumed = Session::resumed("5b0c1f7e-8d43-4a77-9a3e-2f0d6c1b9e42");

        assert_eq!(pinned.id, resumed.id);
        assert!(!pinned.resume);
        assert!(resumed.resume);
    }

    #[test]
    fn a_session_file_is_carried_between_two_homes_under_the_same_path() {
        let homes = Homes::new();
        let layouts = [
            (AgentKind::Claude, CLAUDE_ID, homes.claude()),
            (AgentKind::Codex, CODEX_ID, homes.codex()),
        ];

        for (agent, id, relative) in layouts {
            homes.write(&relative);

            let carried = homes.carry(agent, id).unwrap();

            let expected = homes.to.path().join(&relative);
            assert_eq!(carried.path, expected, "{agent} keeps its relative path");
            assert_eq!(carried.bytes, TRANSCRIPT.len() as u64);
            assert_eq!(fs::read(&expected).unwrap(), TRANSCRIPT);
            assert_eq!(mode(&expected), FILE_MODE, "{agent}'s copy is private");
            assert_eq!(
                homes.locate(homes.to.path(), agent, homes.work.path(), id),
                Some(expected),
                "{agent} finds the carried session where it looks for its own"
            );
        }
    }

    /// Codex keeps a rollout in a day's folder, so one anywhere else under
    /// its sessions folder is none of its sessions.
    #[test]
    fn a_rollout_outside_a_days_folder_is_not_a_codex_session() {
        let homes = Homes::new();
        let name = homes.codex().file_name().unwrap().to_owned();
        for misplaced in [
            Path::new("sessions").join(&name),
            Path::new("sessions/2026/09").join(&name),
            Path::new(CODEX_DAY).join("deeper").join(&name),
        ] {
            homes.write(&misplaced);

            assert_eq!(
                homes.locate(
                    homes.from.path(),
                    AgentKind::Codex,
                    homes.work.path(),
                    CODEX_ID
                ),
                None,
                "{}",
                misplaced.display()
            );
        }

        homes.write(&homes.codex());
        assert_eq!(
            homes.locate(
                homes.from.path(),
                AgentKind::Codex,
                homes.work.path(),
                CODEX_ID
            ),
            Some(homes.from.path().join(homes.codex()))
        );
    }

    #[test]
    fn the_directories_a_carry_creates_are_private() {
        let homes = Homes::new();
        let relative = homes.codex();
        homes.write(&relative);

        homes.carry(AgentKind::Codex, CODEX_ID).unwrap();

        let mut path = homes.to.path().to_path_buf();
        for component in relative.parent().unwrap().components() {
            path.push(component);
            assert_eq!(mode(&path), 0o700, "{} is private", path.display());
        }
    }

    #[test]
    fn a_carried_session_replaces_an_older_copy_and_leaves_nothing_staged() {
        let homes = Homes::new();
        let relative = homes.claude();
        homes.write(&relative);
        let stale = homes.to.path().join(&relative);
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, b"stale").unwrap();

        homes.carry(AgentKind::Claude, CLAUDE_ID).unwrap();

        assert_eq!(fs::read(&stale).unwrap(), TRANSCRIPT);
        let left: Vec<_> = fs::read_dir(stale.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, [stale.file_name().unwrap().to_owned()]);
    }

    #[test]
    fn carrying_a_session_that_does_not_exist_is_reported_not_faked() {
        let homes = Homes::new();
        homes.write(&homes.codex());

        for (agent, id) in [
            (AgentKind::Claude, CLAUDE_ID),
            (AgentKind::Codex, "019b2c41-0000-7000-8000-00000000ffff"),
        ] {
            let error = homes.carry(agent, id).unwrap_err();
            assert!(
                matches!(&error, SessionError::Missing { agent: missing, id: named } if *missing == agent && named == id),
                "{agent}: {error:?}"
            );
        }
        assert!(
            empty(homes.to.path()),
            "nothing is written for a missing session"
        );
    }

    #[test]
    fn a_claude_session_moves_to_the_folder_of_the_directory_its_run_starts_in() {
        let homes = Homes::new();
        let left = homes.claude();
        homes.write(&left);
        let project = TempDir::new().unwrap();
        let expected = homes
            .from
            .path()
            .join(folder(project.path()))
            .join(format!("{CLAUDE_ID}{EXTENSION}"));

        homes
            .relocate(AgentKind::Claude, project.path(), CLAUDE_ID)
            .unwrap();

        assert_eq!(fs::read(&expected).unwrap(), TRANSCRIPT);
        assert!(
            !homes.from.path().join(&left).exists(),
            "one session stays in one place"
        );
        assert_eq!(
            homes.locate(
                homes.from.path(),
                AgentKind::Claude,
                project.path(),
                CLAUDE_ID
            ),
            Some(expected.clone())
        );

        homes
            .relocate(AgentKind::Claude, homes.work.path(), CLAUDE_ID)
            .unwrap();
        assert_eq!(fs::read(homes.from.path().join(&left)).unwrap(), TRANSCRIPT);
        assert!(!expected.exists(), "and it can move back");
    }

    #[test]
    fn a_session_in_place_is_left_alone_and_a_codex_session_is_found_from_anywhere() {
        let homes = Homes::new();
        let claude = homes.claude();
        let codex = homes.codex();
        homes.write(&claude);
        homes.write(&codex);
        let elsewhere = TempDir::new().unwrap();

        homes
            .relocate(AgentKind::Claude, homes.work.path(), CLAUDE_ID)
            .unwrap();
        homes
            .relocate(AgentKind::Codex, elsewhere.path(), CODEX_ID)
            .unwrap();

        assert!(homes.from.path().join(&claude).exists());
        assert!(homes.from.path().join(&codex).exists());
        assert!(
            homes
                .locate(
                    homes.from.path(),
                    AgentKind::Codex,
                    elsewhere.path(),
                    CODEX_ID
                )
                .is_some()
        );
    }

    #[test]
    fn moving_a_session_that_does_not_exist_is_reported() {
        let homes = Homes::new();
        assert!(matches!(
            homes.relocate(AgentKind::Claude, homes.work.path(), CLAUDE_ID),
            Err(SessionError::Missing { .. })
        ));
    }

    /// Carrying a session leaves the copy it came from, so a home can hold an
    /// old one in the very folder the conversation comes back to. Resuming
    /// that would lose every turn since.
    #[test]
    fn the_newest_copy_of_a_session_is_the_one_a_move_keeps() {
        let homes = Homes::new();
        let stale = homes.claude();
        homes.write(&stale);
        let elsewhere = TempDir::new().unwrap();
        let current = folder(elsewhere.path()).join(format!("{CLAUDE_ID}{EXTENSION}"));
        let path = homes.from.path().join(&current);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"current").unwrap();
        age(&homes.from.path().join(&stale));

        homes
            .relocate(AgentKind::Claude, homes.work.path(), CLAUDE_ID)
            .unwrap();

        assert_eq!(
            fs::read(homes.from.path().join(&stale)).unwrap(),
            b"current"
        );
        assert!(!path.exists(), "one session stays in one place");
    }

    #[test]
    fn a_stale_copy_elsewhere_does_not_outlive_the_move_or_take_its_place() {
        let homes = Homes::new();
        let current = homes.claude();
        homes.write(&current);
        let elsewhere = TempDir::new().unwrap();
        let stale = homes
            .from
            .path()
            .join(folder(elsewhere.path()))
            .join(format!("{CLAUDE_ID}{EXTENSION}"));
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, b"stale").unwrap();
        age(&stale);

        homes
            .relocate(AgentKind::Claude, homes.work.path(), CLAUDE_ID)
            .unwrap();

        assert_eq!(
            fs::read(homes.from.path().join(&current)).unwrap(),
            TRANSCRIPT
        );
        assert!(!stale.exists(), "a stale copy could be resumed later");
    }

    /// A copy left unread is not a copy cleaned up, so a move that cannot
    /// read every folder a stale copy may sit in fails rather than leave one
    /// to be resumed later.
    #[test]
    fn a_move_that_cannot_read_a_folder_a_copy_may_sit_in_fails() {
        let homes = Homes::new();
        let current = homes.claude();
        homes.write(&current);
        let elsewhere = TempDir::new().unwrap();
        let unreadable = homes.from.path().join(folder(elsewhere.path()));
        let stale = unreadable.join(format!("{CLAUDE_ID}{EXTENSION}"));
        fs::create_dir_all(&unreadable).unwrap();
        fs::write(&stale, b"stale").unwrap();
        age(&stale);
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();

        let moved = homes.relocate(AgentKind::Claude, homes.work.path(), CLAUDE_ID);

        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(moved, Err(SessionError::Filesystem { .. })),
            "{moved:?}"
        );
        assert!(stale.exists());
    }

    /// The newest copy is the session, so a move that cannot read a folder
    /// the newest may sit in moves nothing rather than an older copy.
    #[test]
    fn a_move_that_cannot_read_every_folder_moves_nothing() {
        let homes = Homes::new();
        let older = TempDir::new().unwrap();
        let older = homes
            .from
            .path()
            .join(folder(older.path()))
            .join(format!("{CLAUDE_ID}{EXTENSION}"));
        fs::create_dir_all(older.parent().unwrap()).unwrap();
        fs::write(&older, b"older").unwrap();
        age(&older);
        let newest = TempDir::new().unwrap();
        let unreadable = homes.from.path().join(folder(newest.path()));
        fs::create_dir_all(&unreadable).unwrap();
        fs::write(
            unreadable.join(format!("{CLAUDE_ID}{EXTENSION}")),
            TRANSCRIPT,
        )
        .unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();

        let moved = homes.relocate(AgentKind::Claude, homes.work.path(), CLAUDE_ID);

        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(moved, Err(SessionError::Filesystem { .. })),
            "{moved:?}"
        );
        assert!(
            older.exists(),
            "an older copy was moved in the newest one's place"
        );
        assert!(!homes.from.path().join(homes.claude()).exists());
    }

    /// A link in Claude's projects folder is no folder of its own and never
    /// holds a copy, so even one that cannot be followed stops no move.
    #[test]
    fn a_link_in_the_projects_folder_stops_no_move() {
        let homes = Homes::new();
        let elsewhere = TempDir::new().unwrap();
        let stale = homes
            .from
            .path()
            .join(folder(elsewhere.path()))
            .join(format!("{CLAUDE_ID}{EXTENSION}"));
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, TRANSCRIPT).unwrap();
        let looped = homes.from.path().join(PROJECTS).join("looped");
        std::os::unix::fs::symlink(&looped, &looped).unwrap();

        homes
            .relocate(AgentKind::Claude, homes.work.path(), CLAUDE_ID)
            .unwrap();

        assert_eq!(
            fs::read(homes.from.path().join(homes.claude())).unwrap(),
            TRANSCRIPT
        );
        assert!(!stale.exists());
    }

    #[test]
    fn the_newest_copy_of_a_session_is_the_one_carried() {
        let homes = Homes::new();
        let stale = homes.claude();
        homes.write(&stale);
        age(&homes.from.path().join(&stale));
        let elsewhere = TempDir::new().unwrap();
        let current = homes
            .from
            .path()
            .join(folder(elsewhere.path()))
            .join(format!("{CLAUDE_ID}{EXTENSION}"));
        fs::create_dir_all(current.parent().unwrap()).unwrap();
        fs::write(&current, b"current").unwrap();

        let carried = homes.carry(AgentKind::Claude, CLAUDE_ID).unwrap();

        assert_eq!(fs::read(&carried.path).unwrap(), b"current");
    }

    /// On a filesystem that ignores case, a folder renamed by case alone
    /// leaves the session in a folder that spells it another way. It is the
    /// same file, and must not be removed as a stale copy of itself.
    #[test]
    fn a_session_in_a_folder_spelled_another_way_is_not_removed_as_a_copy_of_itself() {
        let homes = Homes::new();
        let parent = TempDir::new().unwrap();
        let work = parent.path().join("code");
        fs::create_dir_all(&work).unwrap();
        let project = Session::project(&work);
        let respelled = project.replacen("-code", "-Code", 1);
        assert_ne!(project, respelled);
        let left = Path::new(PROJECTS)
            .join(&respelled)
            .join(format!("{CLAUDE_ID}{EXTENSION}"));
        homes.write(&left);

        homes.relocate(AgentKind::Claude, &work, CLAUDE_ID).unwrap();

        assert_eq!(
            fs::read(
                homes
                    .from
                    .path()
                    .join(PROJECTS)
                    .join(&project)
                    .join(format!("{CLAUDE_ID}{EXTENSION}"))
            )
            .unwrap(),
            TRANSCRIPT,
            "the only copy of the session is gone"
        );
    }

    #[test]
    fn a_session_id_that_names_a_path_is_never_moved() {
        let homes = Homes::new();
        let outside = homes.from.path().join("secret.jsonl");
        fs::write(&outside, TRANSCRIPT).unwrap();

        assert!(matches!(
            homes.relocate(AgentKind::Claude, homes.work.path(), "../../secret"),
            Err(SessionError::Invalid { id }) if id == "../../secret"
        ));
        assert!(outside.exists());
    }

    #[test]
    fn a_session_is_carried_to_another_home_from_the_directory_it_last_ran_in() {
        let homes = Homes::new();
        let previous = TempDir::new().unwrap();
        let left = folder(previous.path()).join(format!("{CLAUDE_ID}{EXTENSION}"));
        homes.write(&left);

        let carried = homes.carry(AgentKind::Claude, CLAUDE_ID).unwrap();

        assert_eq!(carried.path, homes.to.path().join(homes.claude()));
        assert_eq!(fs::read(&carried.path).unwrap(), TRANSCRIPT);
    }

    #[test]
    fn a_session_id_that_names_a_path_is_never_carried_or_found() {
        let homes = Homes::new();
        let outside = homes.from.path().join("secret.jsonl");
        fs::write(&outside, TRANSCRIPT).unwrap();

        for &agent in AgentKind::ALL {
            for id in ["", "../../secret", "../secret", "a/b"] {
                assert!(
                    matches!(homes.carry(agent, id), Err(SessionError::Invalid { .. })),
                    "{agent} refuses {id:?}"
                );
                assert_eq!(
                    homes.locate(homes.from.path(), agent, homes.work.path(), id),
                    None,
                    "{agent} finds {id:?}"
                );
            }
        }
        assert!(empty(homes.to.path()));
    }

    /// A pipe swapped in for a session's file after it was found would hold
    /// the copy open until something wrote to it.
    #[test]
    fn a_pipe_in_a_sessions_place_is_never_waited_on_or_copied() {
        let homes = Homes::new();
        let pipe = homes.from.path().join("session.jsonl");
        mkfifo(&pipe, Mode::S_IRUSR | Mode::S_IWUSR).expect("a pipe named as a session");
        let destination = homes.to.path().join("session.jsonl");
        let (sender, receiver) = mpsc::channel();
        let copied = destination.clone();
        std::thread::spawn(move || {
            let _ = sender.send(copy(&pipe, &copied).map_err(|error| error.kind()));
        });

        let copied = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("the copy waited on a pipe");

        assert_eq!(copied, Err(std::io::ErrorKind::InvalidInput));
        assert!(!destination.exists());
        assert!(empty(homes.to.path()));
    }

    #[test]
    fn a_session_linked_out_of_the_home_is_not_carried() {
        let homes = Homes::new();
        let outside = homes.work.path().join("elsewhere.jsonl");
        fs::write(&outside, TRANSCRIPT).unwrap();

        for (agent, id, relative) in [
            (AgentKind::Claude, CLAUDE_ID, homes.claude()),
            (AgentKind::Codex, CODEX_ID, homes.codex()),
        ] {
            let path = homes.from.path().join(&relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            symlink(&outside, &path).unwrap();

            assert!(
                matches!(homes.carry(agent, id), Err(SessionError::Missing { .. })),
                "{agent} does not follow the link"
            );
        }
        assert!(empty(homes.to.path()));
    }

    #[test]
    fn a_session_under_a_linked_directory_of_the_home_is_not_carried() {
        let homes = Homes::new();
        let outside = TempDir::new().unwrap();

        for (agent, id, relative, linked) in [
            (AgentKind::Claude, CLAUDE_ID, homes.claude(), 2),
            (AgentKind::Codex, CODEX_ID, homes.codex(), 1),
        ] {
            let directory: PathBuf = relative.components().take(linked).collect();
            let target = outside.path().join(agent.as_str());
            let file = target.join(relative.strip_prefix(&directory).unwrap());
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, TRANSCRIPT).unwrap();
            let link = homes.from.path().join(&directory);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(&target, &link).unwrap();

            assert_eq!(
                homes.locate(homes.from.path(), agent, homes.work.path(), id),
                None,
                "{agent} finds a session through a linked directory"
            );
            assert!(
                matches!(homes.carry(agent, id), Err(SessionError::Missing { .. })),
                "{agent} carries a session through a linked directory"
            );
        }
        assert!(empty(homes.to.path()));
    }

    #[test]
    fn a_session_is_never_carried_through_a_linked_directory_of_the_receiving_home() {
        let homes = Homes::new();
        let outside = TempDir::new().unwrap();

        for (agent, id, relative, linked) in [
            (AgentKind::Claude, CLAUDE_ID, homes.claude(), 1),
            (AgentKind::Codex, CODEX_ID, homes.codex(), 2),
        ] {
            homes.write(&relative);
            let directory: PathBuf = relative.components().take(linked).collect();
            let target = outside.path().join(agent.as_str());
            fs::create_dir_all(&target).unwrap();
            let link = homes.to.path().join(&directory);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(&target, &link).unwrap();

            assert!(
                matches!(homes.carry(agent, id), Err(SessionError::Filesystem { .. })),
                "{agent} carries into a linked directory"
            );
            assert!(empty(&target), "{agent} wrote outside the receiving home");
        }
    }

    #[test]
    fn a_claude_session_is_found_under_the_real_path_of_a_linked_work_directory() {
        let homes = Homes::new();
        let relative = homes.claude();
        homes.write(&relative);
        let link = homes.to.path().join("work");
        symlink(homes.work.path(), &link).unwrap();

        assert_eq!(
            homes.locate(homes.from.path(), AgentKind::Claude, &link, CLAUDE_ID),
            Some(homes.from.path().join(relative))
        );
        assert_eq!(Session::project(&link), Session::project(homes.work.path()));
    }

    #[test]
    fn a_work_directory_sanitises_like_claudes_project_folder() {
        let long = format!(
            "/srv/host/state/{}claude/work",
            "3f2b9c1e-6d4a-4f0b-9c7e-1a2b3c4d5e6f/".repeat(6)
        );
        let astral = format!("/var/lib/host/agents/{}/🦀/work", "a".repeat(190));
        let cases = [
            (
                "/srv/host/state/3f2b9c1e-6d4a-4f0b-9c7e-1a2b3c4d5e6f/claude/work",
                "-srv-host-state-3f2b9c1e-6d4a-4f0b-9c7e-1a2b3c4d5e6f-claude-work".to_string(),
            ),
            (
                "/Users/someone/My Projects/.claude/café_☕/work",
                "-Users-someone-My-Projects--claude-caf----work".to_string(),
            ),
            ("/srv/🦀/work", "-srv----work".to_string()),
            (
                long.as_str(),
                format!(
                    "-srv-host-state-{}3f2b9c1e-6d4a-4f0b-9c7e-1a2b3c4d5e6f-guxy8s",
                    "3f2b9c1e-6d4a-4f0b-9c7e-1a2b3c4d5e6f-".repeat(4)
                ),
            ),
            (
                astral.as_str(),
                format!("-var-lib-host-agents-{}-erkwyy", "a".repeat(179)),
            ),
        ];

        for (work, expected) in cases {
            assert_eq!(sanitized(Path::new(work)), expected, "{work}");
        }
    }
}
