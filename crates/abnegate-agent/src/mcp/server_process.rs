use tokio::process::Child;
use tokio::runtime::Handle;

use crate::tool::process::Group;

/// A stdio server's process, which leads a process group of its own.
///
/// Nothing reaps the server while this holds it, even once it has exited by
/// itself: an unreaped server keeps its pid, and so its group's id, from
/// being given to anyone else, so however long a session outlives its server,
/// the kill its drop sends can reach only the server's own group. Dropping
/// this kills that group and only then reaps the server, on the runtime when
/// there is one, where the kill is left to finish repeating itself first.
#[derive(Debug)]
pub(super) struct ServerProcess {
    server: Option<Child>,
    group: Group,
}

impl ServerProcess {
    /// Hold `server`, which has to have been spawned with `process_group(0)`.
    pub(super) fn new(server: Child) -> Self {
        let group = Group::led_by(server.id());
        Self {
            server: Some(server),
            group,
        }
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let Some(mut server) = self.server.take() else {
            return;
        };
        let mut group = std::mem::replace(&mut self.group, Group::led_by(None));
        match Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    group.kill_until_gone().await;
                    let _ = server.wait().await;
                });
            }
            Err(_) => group.kill(),
        }
    }
}
