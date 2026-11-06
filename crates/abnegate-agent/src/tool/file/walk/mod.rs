mod abandon;
mod visit;

use std::collections::HashSet;
use std::fs;
use std::fs::DirEntry;
use std::fs::FileType;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Instant;

use abandon::Abandon;
pub(super) use visit::Visit;

use super::identity::Identity;
use super::withheld::Withheld;
use crate::tool::ToolContext;
use crate::tool::ToolError;

/// Deepest a walk descends below the directory it started from.
pub(super) const MAXIMUM_WALK_DEPTH: usize = 64;

/// Most entries one walk looks at before it gives up.
pub(super) const MAXIMUM_WALK_ENTRIES: usize = 100_000;

pub(super) const OUT_OF_TIME: &str = "out of time";

/// Why a walk, or a search, reports only part of the tree when a directory
/// or a file in it could not be read.
pub(super) const UNREADABLE: &str = "some files could not be read";

const ABANDONED: &str = "abandoned";

const TOO_MANY_ENTRIES: &str = "too many entries";

const TOO_DEEP: &str = "too deep";

/// A depth-first walk of a directory tree that never follows a link.
///
/// A symlinked directory is shown to the visitor as the link it is and never
/// entered, and a directory reached twice by any route (a bind mount, a
/// hard-linked directory) is entered once, so a tree that loops back on
/// itself still ends. An entry the context withholds is neither shown nor
/// entered, whatever name the walk reaches it by. Depth, entry and time
/// budgets bound whatever is left, and a walk whose caller gives up stops at
/// its next entry.
pub(super) struct Walk {
    abandoned: Arc<AtomicBool>,
    deadline: Instant,
    remaining: usize,
    visited: HashSet<Identity>,
    withheld: Withheld,
    stopped: Option<&'static str>,
    unreadable: bool,
}

impl Walk {
    /// A walk that stops once `deadline` passes or `abandoned` is raised.
    pub(super) fn new(
        deadline: Instant,
        abandoned: Arc<AtomicBool>,
        context: &ToolContext,
    ) -> Self {
        Self {
            abandoned,
            deadline,
            remaining: MAXIMUM_WALK_ENTRIES,
            visited: HashSet::new(),
            withheld: Withheld::of(context),
            stopped: None,
            unreadable: false,
        }
    }

    /// Why the walk ended before it had seen the whole tree, if it did.
    pub(super) fn stopped(&self) -> Option<&'static str> {
        self.stopped
    }

    /// Whether an entry, or a directory the visitor asked to enter, could not
    /// be read and was left out.
    pub(super) fn unreadable(&self) -> bool {
        self.unreadable
    }

    /// Show `visit` every entry beneath `root`, entering the directories it
    /// asks to. A visit that ends past the walk's time stops it, out of time,
    /// even when it was the last entry left.
    ///
    /// Only a `root` that cannot be read is an error: a directory further down
    /// that cannot be read is left out, as the entries it held would be, and
    /// [`unreadable`](Self::unreadable) says so.
    pub(super) fn run(
        &mut self,
        root: &Path,
        mut visit: impl FnMut(&DirEntry, FileType) -> Visit,
    ) -> Result<(), ToolError> {
        let entries = fs::read_dir(root)
            .map_err(|error| ToolError::Execution(format!("Cannot read directory: {error}")))?;
        if let Some(identity) = Identity::of(root) {
            self.visited.insert(identity);
        }

        let mut pending: Vec<(fs::ReadDir, usize)> = vec![(entries, 0)];
        while let Some((entries, depth)) = pending.last_mut() {
            let depth = *depth;
            let Some(entry) = entries.next() else {
                pending.pop();
                continue;
            };
            if let Some(reason) = self.exhausted() {
                self.stopped = Some(reason);
                return Ok(());
            }
            self.remaining -= 1;

            let Some((entry, file_type)) = entry
                .ok()
                .and_then(|entry| entry.file_type().ok().map(|file_type| (entry, file_type)))
            else {
                self.unreadable = true;
                continue;
            };
            if self.withholds(&entry) {
                continue;
            }
            match visit(&entry, file_type) {
                Visit::Stop => return Ok(()),
                Visit::Skip => {}
                Visit::Descend if file_type.is_dir() => {
                    if let Some(child) = self.enter(entry.path(), depth) {
                        pending.push((child, depth + 1));
                    }
                }
                Visit::Descend => {}
            }
            if let Some(reason) = self.overdue() {
                self.stopped = Some(reason);
                return Ok(());
            }
        }
        Ok(())
    }

    fn exhausted(&self) -> Option<&'static str> {
        if self.remaining == 0 {
            return Some(TOO_MANY_ENTRIES);
        }
        self.overdue()
    }

    /// Why the walk may not go on, whatever is left of its entries: its
    /// caller gave up, or its time ran out.
    fn overdue(&self) -> Option<&'static str> {
        if self.abandoned.load(Ordering::Relaxed) {
            return Some(ABANDONED);
        }
        if Instant::now() >= self.deadline {
            return Some(OUT_OF_TIME);
        }
        None
    }

    /// Whether the context withholds `entry` itself, judged by what it is on
    /// disk rather than by its name.
    fn withholds(&self, entry: &DirEntry) -> bool {
        entry.metadata().is_ok_and(|metadata| {
            self.withheld
                .holds_entry(&entry.path(), Identity::from(&metadata))
        })
    }

    fn enter(&mut self, directory: PathBuf, depth: usize) -> Option<fs::ReadDir> {
        if depth + 1 > MAXIMUM_WALK_DEPTH {
            self.stopped = Some(TOO_DEEP);
            return None;
        }
        let Ok(metadata) = fs::symlink_metadata(&directory) else {
            self.unreadable = true;
            return None;
        };
        let identity = Identity::from(&metadata);
        if !metadata.is_dir()
            || self.withheld.holds_entry(&directory, identity)
            || !self.visited.insert(identity)
        {
            return None;
        }
        let entries = fs::read_dir(directory).ok();
        self.unreadable |= entries.is_none();
        entries
    }
}

/// Run `work` on a walk that stops at `deadline`, and on a copy of the
/// context, on the blocking pool, where reading a large tree holds a
/// thread of its own rather than the async worker the caller's other tasks
/// are waiting on.
///
/// A blocking thread cannot be cancelled, so a caller that stops waiting,
/// dropping this future, abandons the walk instead, and it ends at its next
/// entry. A walk that never finishes is reported as the `operation`, such as
/// a listing or a search, that did not.
pub(super) async fn walking<Value: Send + 'static>(
    operation: &'static str,
    deadline: Instant,
    context: &ToolContext,
    work: impl FnOnce(&mut Walk, &ToolContext) -> Value + Send + 'static,
) -> Result<Value, ToolError> {
    let abandoned = Arc::new(AtomicBool::new(false));
    let _abandon = Abandon(Arc::clone(&abandoned));
    let context = context.clone();
    tokio::task::spawn_blocking(move || {
        let mut walk = Walk::new(deadline, abandoned, &context);
        work(&mut walk, &context)
    })
    .await
    .map_err(|error| ToolError::Execution(format!("The {operation} did not finish: {error}")))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::sync::mpsc;
    use std::time::Duration;

    use tokio::sync::oneshot;

    use super::*;
    use crate::test_support::PATIENCE;

    const OPERATION: &str = "walk";

    /// A deadline no test reaches.
    fn unreached() -> Instant {
        Instant::now() + PATIENCE
    }

    /// A walk given `limit` from now that nobody abandons.
    fn walk_for(limit: Duration, context: &ToolContext) -> Walk {
        Walk::new(Instant::now() + limit, Arc::default(), context)
    }

    fn names(root: &Path) -> (Vec<String>, Option<&'static str>) {
        names_withholding(root, &ToolContext::default())
    }

    fn names_withholding(
        root: &Path,
        context: &ToolContext,
    ) -> (Vec<String>, Option<&'static str>) {
        let mut walk = walk_for(context.search_timeout, context);
        let mut seen = Vec::new();
        walk.run(root, |entry, _| {
            seen.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .display()
                    .to_string(),
            );
            Visit::Descend
        })
        .unwrap();
        seen.sort();
        (seen, walk.stopped())
    }

    #[test]
    fn a_link_to_a_directory_is_seen_and_never_entered() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("real")).unwrap();
        std::fs::write(root.path().join("real/file"), "x").unwrap();
        symlink(root.path().join("real"), root.path().join("alias")).unwrap();

        let (seen, stopped) = names(root.path());

        assert_eq!(seen, ["alias", "real", "real/file"]);
        assert_eq!(stopped, None);
    }

    #[test]
    fn a_walk_past_its_depth_says_so() {
        let root = tempfile::tempdir().unwrap();
        let mut deep = root.path().to_path_buf();
        for _ in 0..=MAXIMUM_WALK_DEPTH {
            deep.push("d");
        }
        std::fs::create_dir_all(&deep).unwrap();

        let (seen, stopped) = names(root.path());

        assert_eq!(seen.len(), MAXIMUM_WALK_DEPTH + 1);
        assert_eq!(stopped, Some("too deep"));
    }

    #[test]
    fn a_walk_out_of_time_says_so() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), "x").unwrap();
        let mut walk = walk_for(Duration::ZERO, &ToolContext::default());

        walk.run(root.path(), |_, _| Visit::Descend).unwrap();

        assert_eq!(walk.stopped(), Some("out of time"));
    }

    /// A walk on a blocking thread outlived a caller that gave up on it,
    /// reading the tree until its own time ran out.
    #[tokio::test]
    async fn a_walk_stops_once_its_caller_gives_up() {
        let (sender, receiver) = mpsc::channel();
        let context = ToolContext::default();

        let walk = walking(OPERATION, unreached(), &context, move |walk, _| {
            while walk.exhausted().is_none() {
                std::thread::sleep(Duration::from_millis(1));
            }
            sender.send(walk.exhausted()).unwrap();
        });
        let _ = tokio::time::timeout(Duration::from_millis(1), walk).await;

        let reason = receiver
            .recv_timeout(PATIENCE)
            .expect("the walk must stop once nobody is waiting for it");
        assert_eq!(reason, Some(ABANDONED));
    }

    /// A walk awaited on a runtime of one worker hears from a task on that
    /// same runtime only once it has started, so it can finish only when it
    /// runs on a thread of its own: a walk run on the worker, even after
    /// yielding it once, holds the worker the task needs to answer.
    #[tokio::test(flavor = "current_thread")]
    async fn a_walk_runs_off_the_worker_that_awaits_it() {
        let (started, on_start) = oneshot::channel();
        let (answer, answered) = mpsc::channel();
        let answerer = tokio::spawn(async move {
            if on_start.await.is_ok() {
                let _ = answer.send(());
            }
        });

        let heard = walking(
            OPERATION,
            unreached(),
            &ToolContext::default(),
            move |_, _| {
                let _ = started.send(());
                answered.recv_timeout(PATIENCE).is_ok()
            },
        )
        .await
        .expect("the walk finishes");

        answerer.await.expect("the answerer runs");
        assert!(heard, "the walk held the worker its answer needed");
    }

    /// A walk that never finishes names the operation, a listing or a
    /// search, that did not.
    #[tokio::test]
    async fn a_walk_that_never_finishes_names_its_operation() {
        let failed = walking("listing", unreached(), &ToolContext::default(), |_, _| {
            panic!("the walk breaks down")
        })
        .await;

        let Err(ToolError::Execution(message)) = failed else {
            panic!("a walk that broke down answered {failed:?}");
        };
        assert!(
            message.starts_with("The listing did not finish: "),
            "{message}"
        );
    }

    #[test]
    fn an_abandoned_walk_reads_no_further() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), "x").unwrap();
        let context = ToolContext::default();
        let walked = |abandoned: bool| {
            let mut walk = Walk::new(
                Instant::now() + context.search_timeout,
                Arc::new(AtomicBool::new(abandoned)),
                &context,
            );
            let mut seen = 0;
            walk.run(root.path(), |_, _| {
                seen += 1;
                Visit::Descend
            })
            .unwrap();
            (seen, walk.stopped())
        };

        assert_eq!(walked(false), (1, None));
        assert_eq!(walked(true), (0, Some(ABANDONED)));
    }

    /// The time was checked only before an entry, so a visit that ran past
    /// it on the last entry left the walk looking complete.
    #[test]
    fn a_walk_whose_last_visit_runs_past_its_time_says_so() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), "x").unwrap();
        let limit = Duration::from_millis(50);
        let mut walk = walk_for(limit, &ToolContext::default());

        walk.run(root.path(), |_, _| {
            std::thread::sleep(limit * 2);
            Visit::Skip
        })
        .unwrap();

        assert_eq!(walk.stopped(), Some("out of time"));
    }

    #[test]
    fn a_denied_directory_is_neither_shown_nor_entered_under_any_name() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("state/inner")).unwrap();
        std::fs::write(root.path().join("state/inner/file"), "x").unwrap();
        std::fs::create_dir(root.path().join("beside")).unwrap();
        std::fs::write(root.path().join("beside/file"), "x").unwrap();
        let aliases = tempfile::tempdir().unwrap();
        symlink(root.path().join("state"), aliases.path().join("alias")).unwrap();
        let context = ToolContext::default().with_denied([aliases.path().join("alias")]);

        let (seen, stopped) = names_withholding(root.path(), &context);

        assert_eq!(seen, ["beside", "beside/file"]);
        assert_eq!(stopped, None);
    }
}
