use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;

use abnegate_llm::ToolCall;
use abnegate_secret::sanitize;
use serde_json::Value;

use crate::activity::subject;
use crate::event::AgentEvent;
use crate::output::Output;
use crate::parser::claude::delegation::Delegation;
use crate::parser::claude::hand_back;
use crate::parser::claude::notice::Notice;
use crate::parser::claude::report::Report;
use crate::parser::claude::task_event::TaskEvent;
use crate::subagent::StepStatus;
use crate::subagent::Subagent;
use crate::subagent::SubagentStatus;
use crate::subagent::SubagentStep;

/// The tools the main agent starts a subagent with. `Task` is the name older
/// releases gave `Agent`.
const AGENT_TOOLS: &[&str] = &["Agent", "Task"];

/// The kinds of task claude runs a subagent as. Every other task, such as a
/// command left running in the background, is no subagent.
const AGENT_TASKS: &[&str] = &["local_agent", "remote_agent"];

/// The status of a finished subagent's report.
const COMPLETED: &str = "completed";

/// The statuses of a call that only launched its subagent, which then runs
/// on in the background, remotely or as a teammate and reports through task
/// notices instead.
const LAUNCHED: &[&str] = &["async_launched", "remote_launched", "teammate_spawned"];

/// claude's words for where a task stands. A paused task is still running,
/// and a killed one was stopped.
const TASK_STATUSES: &[(&str, SubagentStatus)] = &[
    ("pending", SubagentStatus::Running),
    ("running", SubagentStatus::Running),
    ("paused", SubagentStatus::Running),
    ("completed", SubagentStatus::Completed),
    ("failed", SubagentStatus::Failed),
    ("killed", SubagentStatus::Stopped),
    ("stopped", SubagentStatus::Stopped),
];

/// The calls a turn's agents made and the subagents the main agent started,
/// kept across the turn's lines, so a tool's result is paired with the call
/// it answers and a subagent's news with the call that started it.
///
/// A subagent's own calls are steps of its run. So are those of every agent
/// nested inside it, and a nested agent's task news is none of the main
/// agent's. A call by an agent no known subagent descends from, such as a
/// forked skill's, is the main agent's.
#[derive(Debug, Clone, Default)]
pub(crate) struct Roster {
    /// Calls still waiting on their result, by id.
    calls: HashMap<String, ToolCall>,
    /// Whether each subagent the main agent started runs in the background,
    /// by the id of the call that started it. A background subagent's call
    /// returns once it is launched, so its result is no report of its work.
    subagents: HashMap<String, bool>,
    /// The subagent each of a subagent's own calls belongs to, by call id.
    steps: HashMap<String, String>,
    /// The call that started each subagent's task, by task id: claude names
    /// only the task on some of its news of one.
    tasks: HashMap<String, String>,
    /// The subagent the main agent started that each agent a subagent
    /// started descends from, by the id of the call that started it.
    nested: HashMap<String, String>,
    /// The subagents whose call returned their full report, which the
    /// shorter summary of a later task notice must not replace.
    concluded: HashSet<String>,
}

impl Roster {
    /// The main agent made `call`, followed by the start of the subagent it
    /// starts, when it starts one.
    pub(crate) fn called(&mut self, call: ToolCall, events: &mut Vec<AgentEvent>) {
        let started = self.started(&call);
        events.push(self.remembered(call));
        events.extend(started.map(AgentEvent::Subagent));
    }

    /// The agent `parent` names made `calls`: steps of the subagent it is or
    /// descends from, or the main agent's calls when it descends from none.
    pub(crate) fn delegated(
        &mut self,
        parent: &str,
        calls: impl IntoIterator<Item = ToolCall>,
        events: &mut Vec<AgentEvent>,
    ) {
        let root = self.root(parent);
        for call in calls {
            let Some(root) = &root else {
                events.push(self.remembered(call));
                continue;
            };
            if AGENT_TOOLS.contains(&call.function.name.as_str()) {
                self.nested.insert(call.id.clone(), root.clone());
            }
            self.steps.insert(call.id.clone(), root.clone());
            events.push(AgentEvent::Subagent(stepped(
                root,
                &call,
                StepStatus::Running,
            )));
            self.calls.insert(call.id.clone(), call);
        }
    }

    /// Each of `results`, a call's id, what it printed and whether it failed,
    /// as the [`Output`] of the call it answers, after what it says of the
    /// subagent the call was a step of or started. `report` is the tool's
    /// account of the call, a subagent's report when it started one: the
    /// account of the only result, or, among several, of the one call the
    /// report's task is known to have been started by, or else of the only
    /// one that started a subagent, and of none when that is not known
    /// either. A result no call is known for is ignored. Only an account
    /// whose call is known, and not just the only one it could be, stands in
    /// for the subagent's answer in place of a wrapper around it.
    pub(crate) fn returned(
        &mut self,
        results: impl IntoIterator<Item = (String, Value, bool)>,
        report: Option<&Report>,
        events: &mut Vec<AgentEvent>,
    ) {
        let results: Vec<(String, Value, bool)> = results.into_iter().collect();
        let known = results.len() == 1
            || report
                .and_then(|report| report.task.as_ref())
                .is_some_and(|task| self.tasks.contains_key(task));
        let owner = match results.as_slice() {
            [(id, _, _)] => Some(id.clone()),
            _ => report
                .and_then(|report| report.task.as_ref())
                .and_then(|task| self.tasks.get(task))
                .cloned()
                .or_else(|| self.only_starter(&results)),
        };
        for (id, content, failed) in results {
            let report = report.filter(|_| owner.as_deref() == Some(id.as_str()));
            let Some(call) = self.calls.remove(&id) else {
                continue;
            };
            let text = printed(&content);
            if let Some(parent) = self.steps.remove(&id) {
                let status = if failed {
                    StepStatus::Failed
                } else {
                    StepStatus::Succeeded
                };
                events.push(AgentEvent::Subagent(stepped(&parent, &call, status)));
            }
            let whole = report
                .filter(|_| known)
                .and_then(|report| report.replacing(&text));
            events.extend(
                self.reported(&id, &text, whole, failed, report)
                    .filter(|update| !update.empty())
                    .map(AgentEvent::Subagent),
            );
            events.push(AgentEvent::Output(Output::new(call, text, !failed)));
        }
    }

    /// The id of the only one of `results` that answers a call starting a
    /// subagent, when exactly one does.
    fn only_starter(&self, results: &[(String, Value, bool)]) -> Option<String> {
        let mut starters = results
            .iter()
            .map(|(id, _, _)| id)
            .filter(|id| self.subagents.contains_key(*id));
        match (starters.next(), starters.next()) {
            (Some(id), None) => Some(id.clone()),
            _ => None,
        }
    }

    /// What a task `event`'s `notice` says of a subagent the main agent
    /// started, when it says anything.
    pub(crate) fn noticed(&mut self, event: TaskEvent, notice: Notice) -> Option<Subagent> {
        let update = match event {
            TaskEvent::Started => self.begun(notice),
            TaskEvent::Progress => self.progressed(notice),
            TaskEvent::Updated => self.patched(notice),
            TaskEvent::Notification => self.notified(notice),
        };
        update.filter(|update| !update.empty())
    }

    fn remembered(&mut self, call: ToolCall) -> AgentEvent {
        self.calls.insert(call.id.clone(), call.clone());
        AgentEvent::Tool(call)
    }

    /// The start of the subagent `call` starts, when it starts one.
    fn started(&mut self, call: &ToolCall) -> Option<Subagent> {
        if !AGENT_TOOLS.contains(&call.function.name.as_str()) {
            return None;
        }
        let delegation: Delegation =
            serde_json::from_str(&call.function.arguments).unwrap_or_default();
        self.subagents.insert(
            call.id.clone(),
            delegation.run_in_background.unwrap_or_default(),
        );
        Some(Subagent {
            description: shown(delegation.description),
            kind: shown(delegation.subagent_type),
            model: shown(delegation.model),
            background: delegation.run_in_background,
            status: Some(SubagentStatus::Running),
            ..Subagent::new(call.id.as_str())
        })
    }

    /// The subagent the main agent started that the agent `parent` names
    /// is, or descends from.
    fn root(&self, parent: &str) -> Option<String> {
        if self.subagents.contains_key(parent) {
            return Some(parent.to_string());
        }
        self.nested.get(parent).cloned()
    }

    /// What the result of the call that started subagent `id` says of it. A
    /// foreground subagent's result is its report; a launched one's says
    /// only that it runs on, and its report comes later as task news. Only a
    /// report that says something outranks a later task notice's summary.
    /// The [`Output`] of the call keeps the frame claude wrote around the
    /// report, or the wrapper it wrote in place of one too large to hold, as
    /// the main agent read it; only this news is without them, and has the
    /// `whole` answer the wrapper stood in place of, when that is known.
    fn reported(
        &mut self,
        id: &str,
        text: &str,
        whole: Option<&str>,
        failed: bool,
        report: Option<&Report>,
    ) -> Option<Subagent> {
        let background = *self.subagents.get(id)?;
        let empty = Report::default();
        let report = report.unwrap_or(&empty);
        let written = whole.map_or_else(|| hand_back::report(text), Cow::Borrowed);
        if let Some(task) = report.task.as_ref().filter(|task| !task.is_empty()) {
            self.tasks
                .entry(task.clone())
                .or_insert_with(|| id.to_string());
        }
        let mut update = Subagent {
            model: shown(report.model.as_deref()),
            ..Subagent::new(id)
        };
        if failed {
            update.status = Some(SubagentStatus::Failed);
            update.error = shown(Some(&written));
            return Some(update);
        }
        match report.status.as_deref() {
            Some(status) if LAUNCHED.contains(&status) => {
                self.subagents.insert(id.to_string(), true);
                update.background = Some(true);
            }
            Some(COMPLETED) => {
                update.status = Some(SubagentStatus::Completed);
                update.tool_uses = report.tool_uses;
                update.duration = report.duration_ms.map(Duration::from_millis);
                update.tokens = report.tokens;
                update.summary = shown(Some(&written));
            }
            None if !background => {
                update.status = Some(SubagentStatus::Completed);
                update.summary = shown(Some(&written));
            }
            Some(_) | None => {}
        }
        if update.summary.is_some() {
            self.concluded.insert(id.to_string());
        }
        Some(update)
    }

    /// A task claude started: a subagent when it is of an agent's kind, or
    /// of no kind and started by a call already known to start one. An agent
    /// a subagent started is none: its calls are its root subagent's steps.
    fn begun(&mut self, task: Notice) -> Option<Subagent> {
        if task.nested() {
            return None;
        }
        let id = task.tool_use_id.filter(|id| !id.is_empty())?;
        if self.nested.contains_key(&id) {
            return None;
        }
        let agent = match task.task_type.as_deref() {
            Some(kind) => AGENT_TASKS.contains(&kind),
            None => self.subagents.contains_key(&id),
        };
        if !agent {
            return None;
        }
        if let Some(task_id) = task.task_id.filter(|task_id| !task_id.is_empty()) {
            self.tasks.insert(task_id, id.clone());
        }
        let background = self.subagents.entry(id.clone()).or_default();
        if let Some(backgrounded) = task.is_backgrounded {
            *background = backgrounded;
        }
        Some(Subagent {
            description: shown(task.description),
            kind: shown(task.subagent_type),
            background: task.is_backgrounded,
            status: Some(SubagentStatus::Running),
            ..Subagent::new(id)
        })
    }

    fn progressed(&self, task: Notice) -> Option<Subagent> {
        let id = self.tracked(&task)?;
        let usage = task.usage.unwrap_or_default();
        Some(Subagent {
            kind: shown(task.subagent_type),
            tool_uses: usage.tool_uses,
            duration: usage.duration_ms.map(Duration::from_millis),
            tokens: usage.total_tokens,
            last_tool: shown(task.last_tool_name),
            summary: self.summary(&id, task.summary),
            ..Subagent::new(id)
        })
    }

    fn patched(&mut self, task: Notice) -> Option<Subagent> {
        let id = self.tracked(&task)?;
        let patch = task.patch?;
        if let Some(backgrounded) = patch.is_backgrounded {
            self.subagents.insert(id.clone(), backgrounded);
        }
        Some(Subagent {
            description: shown(patch.description),
            background: patch.is_backgrounded,
            status: patch.status.as_deref().and_then(task_status),
            error: shown(patch.error),
            ..Subagent::new(id)
        })
    }

    fn notified(&self, task: Notice) -> Option<Subagent> {
        let id = self.tracked(&task)?;
        let usage = task.usage.unwrap_or_default();
        Some(Subagent {
            status: task.status.as_deref().and_then(task_status),
            tool_uses: usage.tool_uses,
            duration: usage.duration_ms.map(Duration::from_millis),
            tokens: usage.total_tokens,
            summary: self.summary(&id, task.summary),
            ..Subagent::new(id)
        })
    }

    /// A task notice's `summary` of subagent `id`, unless its call already
    /// returned the full report: claude falls back to the task's description
    /// for a summary it has none of.
    fn summary(&self, id: &str, summary: Option<String>) -> Option<String> {
        if self.concluded.contains(id) {
            return None;
        }
        shown(summary)
    }

    /// The subagent news of a task is about: the call that started it, named
    /// outright or through its task. A task that is no known subagent, such
    /// as a command run in the background, is none.
    fn tracked(&self, task: &Notice) -> Option<String> {
        let id = task
            .tool_use_id
            .as_ref()
            .filter(|id| !id.is_empty())
            .or_else(|| self.tasks.get(task.task_id.as_deref()?))?;
        self.subagents.contains_key(id).then(|| id.clone())
    }
}

/// A tool result's text, which claude writes either as one string or as a
/// list of blocks of which only the text ones say anything.
fn printed(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// An agent's own words made safe to show, or nothing when they say nothing.
fn shown(text: Option<impl AsRef<str>>) -> Option<String> {
    let text = text?;
    let text = sanitize(text.as_ref());
    (!text.trim().is_empty()).then(|| text.into_owned())
}

/// The news of one of `parent`'s own calls standing at `status`. Only a
/// call just made is the subagent's latest tool.
fn stepped(parent: &str, call: &ToolCall, status: StepStatus) -> Subagent {
    let name = &call.function.name;
    let mut step = SubagentStep::new(call.id.as_str(), name.as_str(), status);
    step.subject = subject(&call.function.arguments);
    Subagent {
        last_tool: (status == StepStatus::Running).then(|| name.clone()),
        step: Some(step),
        ..Subagent::new(parent)
    }
}

fn task_status(name: &str) -> Option<SubagentStatus> {
    TASK_STATUSES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, status)| *status)
}
