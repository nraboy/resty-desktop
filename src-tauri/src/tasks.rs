//! Unified operation lifecycle event bus.
//!
//! Every restic-shelling operation (backup, restore, copy, mirror, prune, forget,
//! check, diff, index, unlock, init) emits a `task` event through this module,
//! alongside — not instead of — its existing detailed feed (e.g. `backup:progress`).
//! See CLAUDE.md's "Operation Event Bus" section for the full design rationale.
//!
//! No frontend logic subscribes to this yet by design (see CLAUDE.md) — it exists
//! so a future background-task consumer has a uniform, `operationId`-correlatable
//! stream to build on, without having to retrofit every operation at that point.

use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

pub const TASK_EVENT: &str = "task";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskKind {
    Backup,
    Restore,
    RestorePath,
    Copy,
    Mirror,
    Prune,
    Forget,
    Tag,
    Check,
    Diff,
    Index,
    Unlock,
    Stats,
    TestConnection,
    Browse,
    // Reserved for repo init/add — not wired to a call site yet (see CLAUDE.md's
    // Operation Event Bus section for the covered-operation list).
    #[allow(dead_code)]
    Init,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskPhase {
    /// An operation that's been created and registered (so it's already visible
    /// and cancellable) but hasn't started its real work yet — currently only
    /// emitted by "Index All" batches waiting their turn on `IndexHandle::batch_turn`
    /// (see browse.rs's `index_snapshots_batch`). Followed by `Started` once the
    /// operation actually begins, or a cancel-path phase if it's stopped while
    /// still pending. Not emitted by any reject-on-busy operation (backup, restore,
    /// prune, copy, mirror) — those have no queued state to represent.
    Pending,
    Started,
    Progress,
    Cancelling,
    Cancelled,
    Finished,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskOrigin {
    Manual,
    Scheduler,
    Background,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgress {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent_done: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items_done: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_done: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    // The four fields below carry per-kind progress detail that the normalized
    // fields above can't hold — kept optional so the bus stays lossless vs the
    // legacy `*:progress` events (backup:progress, restore:progress — prune:progress
    // was retired once its one remaining consumer, SettingsPage's "Prune All" modal,
    // moved onto the `task` bus) even though no consumer reads them yet. See
    // CLAUDE.md's Operation Event Bus section.
    /// Backup and restore: elapsed time since the operation started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds_elapsed: Option<u64>,
    /// Backup only: restic's ETA for completion, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds_remaining: Option<u64>,
    /// Backup only: file paths currently being scanned/uploaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_files: Option<Vec<String>>,
    /// Prune-all only: the repo this tick's progress applies to, distinct from
    /// the envelope's top-level `repo_id` (intentionally "" for a multi-repo
    /// prune-all, since there's no single repo for the whole operation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub operation_id: String,
    pub kind: TaskKind,
    pub phase: TaskPhase,
    pub repo_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    pub origin: TaskOrigin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<TaskProgress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub at: i64,
}

/// Minimal identity a `cancel_*` command needs to emit a `Cancelling` event for
/// whatever operation is currently running on a given handle.
#[derive(Debug, Clone)]
pub struct TaskRef {
    pub operation_id: String,
    pub kind: TaskKind,
    pub repo_id: String,
    pub target_id: Option<String>,
    // So emit_cancelling can report the operation's real origin instead of
    // assuming every cancel-path caller is manual — a scheduler-triggered
    // backup stopped via the same cancel_backup path must keep reporting
    // origin: scheduler across its whole started->cancelling->cancelled run.
    pub origin: TaskOrigin,
}

/// Slot a cancellable handle (BackupHandle, RestoreHandle, CopyHandle,
/// MirrorHandle, PruneHandle) carries so its `cancel_*` command can find the
/// currently-running operation's identity. `None` when idle.
pub type TaskSlot = Arc<Mutex<Option<TaskRef>>>;

pub fn new_task_slot() -> TaskSlot {
    Arc::new(Mutex::new(None))
}

/// Emission seam: lets tests record emitted events without a real `AppHandle`.
pub trait TaskSink: Send + Sync {
    fn send(&self, ev: &TaskEvent);
}

impl TaskSink for AppHandle {
    fn send(&self, ev: &TaskEvent) {
        let _ = self.emit(TASK_EVENT, ev);
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Same 16-char alphanumeric scheme already used for `backup_history.id` in
/// `execute_backup` (snapshot.rs) — reused rather than adding a new crate/scheme.
pub fn new_operation_id() -> String {
    use rand::Rng;
    rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(16)
        .map(char::from)
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn build_event(
    operation_id: &str,
    kind: TaskKind,
    phase: TaskPhase,
    repo_id: &str,
    target_id: Option<String>,
    origin: TaskOrigin,
    progress: Option<TaskProgress>,
    error: Option<String>,
) -> TaskEvent {
    TaskEvent {
        operation_id: operation_id.to_string(),
        kind,
        phase,
        repo_id: repo_id.to_string(),
        target_id,
        origin,
        progress,
        error,
        at: now_millis(),
    }
}

/// Cheap, `Clone`-able progress emitter — the piece that gets moved into a
/// `spawn_blocking` closure so streaming ops can call `.emit(...)` per status
/// line without holding the `OperationCtx` itself (which owns the terminal
/// transition and must stay in the outer async scope to read the final `Result`).
#[derive(Clone)]
pub struct TaskProgressEmitter<S: TaskSink + Clone> {
    sink: S,
    operation_id: String,
    kind: TaskKind,
    repo_id: String,
    target_id: Option<String>,
    origin: TaskOrigin,
}

impl<S: TaskSink + Clone> TaskProgressEmitter<S> {
    pub fn emit(&self, progress: TaskProgress) {
        let ev = build_event(
            &self.operation_id,
            self.kind,
            TaskPhase::Progress,
            &self.repo_id,
            self.target_id.clone(),
            self.origin,
            Some(progress),
            None,
        );
        self.sink.send(&ev);
    }
}

/// Owns one operation's lifecycle. Emits `Started` on construction; exactly one
/// of `.finished()` / `.failed()` / `.cancelled()` should be called on every
/// normal exit path. If none is called (early return via `?`, panic unwind), the
/// `Drop` impl emits a trailing `Failed("operation dropped")` so the operation
/// never silently vanishes from the bus — a backstop, not the intended path.
pub struct OperationCtx<S: TaskSink + Clone> {
    sink: S,
    operation_id: String,
    kind: TaskKind,
    repo_id: String,
    target_id: Option<String>,
    origin: TaskOrigin,
    slot: Option<TaskSlot>,
    terminal: AtomicBool,
}

impl<S: TaskSink + Clone> OperationCtx<S> {
    /// `slot`: pass the owning handle's `TaskSlot` for cancellable operations so
    /// `cancel_*` can find this operation's `TaskRef` and emit `Cancelling`; pass
    /// `None` for operations with no cancel path (check, diff, tag, unlock, etc.).
    pub fn new(
        sink: S,
        kind: TaskKind,
        repo_id: impl Into<String>,
        target_id: Option<String>,
        origin: TaskOrigin,
        slot: Option<TaskSlot>,
    ) -> Self {
        Self::new_with_initial_phase(sink, kind, repo_id, target_id, origin, slot, TaskPhase::Started)
    }

    /// Like `new`, but the operation isn't doing its real work yet — it's queued,
    /// waiting for some internal turn (currently only "Index All" batches waiting
    /// on `IndexHandle::batch_turn`). Emits `Pending` instead of `Started`, and
    /// still registers `slot` so it's cancellable while queued. Call `.activate()`
    /// once the operation actually begins.
    pub fn new_pending(
        sink: S,
        kind: TaskKind,
        repo_id: impl Into<String>,
        target_id: Option<String>,
        origin: TaskOrigin,
        slot: Option<TaskSlot>,
    ) -> Self {
        Self::new_with_initial_phase(sink, kind, repo_id, target_id, origin, slot, TaskPhase::Pending)
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_initial_phase(
        sink: S,
        kind: TaskKind,
        repo_id: impl Into<String>,
        target_id: Option<String>,
        origin: TaskOrigin,
        slot: Option<TaskSlot>,
        initial_phase: TaskPhase,
    ) -> Self {
        let operation_id = new_operation_id();
        let repo_id = repo_id.into();

        if let Some(s) = &slot {
            if let Ok(mut guard) = s.lock() {
                *guard = Some(TaskRef {
                    operation_id: operation_id.clone(),
                    kind,
                    repo_id: repo_id.clone(),
                    target_id: target_id.clone(),
                    origin,
                });
            }
        }

        let ev = build_event(
            &operation_id,
            kind,
            initial_phase,
            &repo_id,
            target_id.clone(),
            origin,
            None,
            None,
        );
        sink.send(&ev);

        Self {
            sink,
            operation_id,
            kind,
            repo_id,
            target_id,
            origin,
            slot,
            terminal: AtomicBool::new(false),
        }
    }

    /// Transitions a `new_pending` operation from queued to actually running by
    /// emitting `Started`. Does not touch the `terminal` flag — `.finished()` /
    /// `.failed()` / `.cancelled()` still work normally afterward. Calling this on
    /// an operation created via `new` (already `Started`) would just emit a
    /// redundant `Started` — harmless, but there's no call site that does that
    /// today.
    pub fn activate(&self) {
        let ev = build_event(
            &self.operation_id,
            self.kind,
            TaskPhase::Started,
            &self.repo_id,
            self.target_id.clone(),
            self.origin,
            None,
            None,
        );
        self.sink.send(&ev);
    }

    /// Exposed for callers that need to correlate the started event's id with the
    /// operation still in flight. Wired: `index_snapshots_batch` (browse.rs) reads this
    /// to register its batch-level cancel flag/task slot in `IndexHandle::batches` under
    /// the same id the `started` event already carries.
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub fn progress_emitter(&self) -> TaskProgressEmitter<S> {
        TaskProgressEmitter {
            sink: self.sink.clone(),
            operation_id: self.operation_id.clone(),
            kind: self.kind,
            repo_id: self.repo_id.clone(),
            target_id: self.target_id.clone(),
            origin: self.origin,
        }
    }

    pub fn finished(self) {
        self.terminate(TaskPhase::Finished, None);
    }

    pub fn failed(self, error: impl Into<String>) {
        self.terminate(TaskPhase::Failed, Some(error.into()));
    }

    pub fn cancelled(self) {
        self.terminate(TaskPhase::Cancelled, None);
    }

    fn terminate(&self, phase: TaskPhase, error: Option<String>) {
        // compare_exchange so a concurrent Drop (unwind) and an explicit terminal
        // call can never both fire — first one wins, matching the busy-guard
        // pattern used elsewhere in this codebase.
        if self
            .terminal
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        if let Some(s) = &self.slot {
            if let Ok(mut guard) = s.lock() {
                *guard = None;
            }
        }
        let ev = build_event(
            &self.operation_id,
            self.kind,
            phase,
            &self.repo_id,
            self.target_id.clone(),
            self.origin,
            None,
            error,
        );
        self.sink.send(&ev);
    }
}

impl<S: TaskSink + Clone> Drop for OperationCtx<S> {
    fn drop(&mut self) {
        // Backstop only — see struct doc comment. Every wired call site calls a
        // terminal method explicitly; reaching here un-terminated means an early
        // return or panic unwind skipped it.
        self.terminate(TaskPhase::Failed, Some("operation dropped".to_string()));
    }
}

/// Emits a `Cancelling` event for whatever operation is currently recorded in
/// `slot`, if any. Called from `cancel_*` commands right before they kill the
/// child process. No-ops (does not error) if the slot is empty — matches the
/// existing `cancel_*` commands' best-effort style.
pub fn emit_cancelling<S: TaskSink>(sink: &S, slot: &TaskSlot) {
    let task_ref = match slot.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => return,
    };
    if let Some(r) = task_ref {
        let ev = build_event(
            &r.operation_id,
            r.kind,
            TaskPhase::Cancelling,
            &r.repo_id,
            r.target_id,
            // Report the operation's real origin (e.g. a scheduler-triggered
            // backup stopped via the same manual cancel_backup path must keep
            // reporting origin: scheduler across started->cancelling->cancelled),
            // not the origin of whoever clicked Stop.
            r.origin,
            None,
            None,
        );
        sink.send(&ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct RecordingSink(Arc<Mutex<Vec<TaskEvent>>>);

    impl RecordingSink {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(Vec::new())))
        }
        fn events(&self) -> Vec<TaskEvent> {
            self.0.lock().unwrap().clone()
        }
    }

    impl TaskSink for RecordingSink {
        fn send(&self, ev: &TaskEvent) {
            self.0.lock().unwrap().push(ev.clone());
        }
    }

    #[test]
    fn started_progress_finished_sequence() {
        let sink = RecordingSink::new();
        let ctx = OperationCtx::new(
            sink.clone(),
            TaskKind::Backup,
            "repo-1",
            Some("plan-1".to_string()),
            TaskOrigin::Manual,
            None,
        );
        let op_id = ctx.operation_id().to_string();
        ctx.progress_emitter().emit(TaskProgress {
            percent_done: Some(0.5),
            ..Default::default()
        });
        ctx.finished();

        let events = sink.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].phase, TaskPhase::Started);
        assert_eq!(events[1].phase, TaskPhase::Progress);
        assert_eq!(events[2].phase, TaskPhase::Finished);
        for ev in &events {
            assert_eq!(ev.operation_id, op_id);
            assert_eq!(ev.kind, TaskKind::Backup);
            assert_eq!(ev.repo_id, "repo-1");
            assert_eq!(ev.target_id.as_deref(), Some("plan-1"));
            assert_eq!(ev.origin, TaskOrigin::Manual);
        }
    }

    #[test]
    fn pending_activate_finished_sequence() {
        let sink = RecordingSink::new();
        let ctx = OperationCtx::new_pending(
            sink.clone(),
            TaskKind::Index,
            "repo-1",
            None,
            TaskOrigin::Manual,
            None,
        );
        let op_id = ctx.operation_id().to_string();
        ctx.activate();
        ctx.progress_emitter().emit(TaskProgress {
            items_done: Some(1),
            ..Default::default()
        });
        ctx.finished();

        let events = sink.events();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].phase, TaskPhase::Pending);
        assert_eq!(events[1].phase, TaskPhase::Started);
        assert_eq!(events[2].phase, TaskPhase::Progress);
        assert_eq!(events[3].phase, TaskPhase::Finished);
        for ev in &events {
            assert_eq!(ev.operation_id, op_id);
        }
    }

    #[test]
    fn pending_cancelled_while_still_queued_never_emits_started() {
        let sink = RecordingSink::new();
        let ctx = OperationCtx::new_pending(
            sink.clone(),
            TaskKind::Index,
            "repo-1",
            None,
            TaskOrigin::Manual,
            None,
        );
        ctx.cancelled();

        let events = sink.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].phase, TaskPhase::Pending);
        assert_eq!(events[1].phase, TaskPhase::Cancelled);
    }

    #[test]
    fn drop_without_terminal_emits_failed() {
        let sink = RecordingSink::new();
        {
            let _ctx = OperationCtx::new(
                sink.clone(),
                TaskKind::Check,
                "repo-1",
                None,
                TaskOrigin::Manual,
                None,
            );
            // dropped without calling finished/failed/cancelled
        }
        let events = sink.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].phase, TaskPhase::Started);
        assert_eq!(events[1].phase, TaskPhase::Failed);
        assert_eq!(events[1].error.as_deref(), Some("operation dropped"));
    }

    #[test]
    fn second_terminal_call_is_noop() {
        let sink = RecordingSink::new();
        let ctx = OperationCtx::new(
            sink.clone(),
            TaskKind::Prune,
            "repo-1",
            None,
            TaskOrigin::Manual,
            None,
        );
        ctx.terminate(TaskPhase::Finished, None);
        ctx.terminate(TaskPhase::Failed, Some("should not appear".to_string()));

        let events = sink.events();
        // started + one terminal only — the second terminate() call is a no-op.
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].phase, TaskPhase::Finished);
    }

    #[test]
    fn slot_is_published_on_start_and_cleared_on_terminal() {
        let sink = RecordingSink::new();
        let slot = new_task_slot();
        let ctx = OperationCtx::new(
            sink.clone(),
            TaskKind::Restore,
            "repo-1",
            Some("snap-1".to_string()),
            TaskOrigin::Manual,
            Some(slot.clone()),
        );
        {
            let guard = slot.lock().unwrap();
            let r = guard.as_ref().expect("slot should be populated after new()");
            assert_eq!(r.kind, TaskKind::Restore);
            assert_eq!(r.repo_id, "repo-1");
            assert_eq!(r.target_id.as_deref(), Some("snap-1"));
        }
        ctx.finished();
        assert!(slot.lock().unwrap().is_none(), "slot should be cleared after terminal");
    }

    #[test]
    fn emit_cancelling_reads_slot_and_emits_once() {
        let sink = RecordingSink::new();
        let slot = new_task_slot();
        let ctx = OperationCtx::new(
            sink.clone(),
            TaskKind::Copy,
            "repo-2",
            None,
            TaskOrigin::Manual,
            Some(slot.clone()),
        );
        emit_cancelling(&sink, &slot);
        let events = sink.events();
        assert_eq!(events.len(), 2); // started, cancelling
        assert_eq!(events[1].phase, TaskPhase::Cancelling);
        assert_eq!(events[1].operation_id, ctx.operation_id());
        ctx.cancelled();
        let events = sink.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[2].phase, TaskPhase::Cancelled);
    }

    #[test]
    fn emit_cancelling_on_empty_slot_is_noop() {
        let sink = RecordingSink::new();
        let slot = new_task_slot();
        emit_cancelling(&sink, &slot);
        assert!(sink.events().is_empty());
    }

    #[test]
    fn build_event_serializes_expected_camel_case_shape() {
        let ev = build_event(
            "abc123",
            TaskKind::RestorePath,
            TaskPhase::Progress,
            "repo-9",
            Some("snap-9".to_string()),
            TaskOrigin::Background,
            Some(TaskProgress {
                percent_done: Some(0.25),
                items_done: Some(1),
                ..Default::default()
            }),
            None,
        );
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["operationId"], "abc123");
        assert_eq!(json["kind"], "restorePath");
        assert_eq!(json["phase"], "progress");
        assert_eq!(json["repoId"], "repo-9");
        assert_eq!(json["targetId"], "snap-9");
        assert_eq!(json["origin"], "background");
        assert_eq!(json["progress"]["percentDone"], 0.25);
        assert_eq!(json["progress"]["itemsDone"], 1);
        // Optional fields left None must be omitted, not emitted as null.
        assert!(json["progress"].get("itemsTotal").is_none());
        assert!(json["progress"].get("secondsElapsed").is_none());
        assert!(json.get("error").is_none());
    }

    #[test]
    fn build_event_serializes_per_kind_progress_detail() {
        // Pins the four fields added so the bus stays lossless vs the legacy
        // backup:progress/restore:progress payloads (see tasks.rs's TaskProgress
        // doc comment) and keeps types.ts in sync.
        let ev = build_event(
            "abc123",
            TaskKind::Backup,
            TaskPhase::Progress,
            "repo-1",
            None,
            TaskOrigin::Manual,
            Some(TaskProgress {
                seconds_elapsed: Some(12),
                seconds_remaining: Some(34),
                current_files: Some(vec!["a.txt".to_string(), "b.txt".to_string()]),
                repo_id: Some("repo-7".to_string()),
                ..Default::default()
            }),
            None,
        );
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["progress"]["secondsElapsed"], 12);
        assert_eq!(json["progress"]["secondsRemaining"], 34);
        assert_eq!(json["progress"]["currentFiles"][0], "a.txt");
        assert_eq!(json["progress"]["currentFiles"][1], "b.txt");
        assert_eq!(json["progress"]["repoId"], "repo-7");
    }

    #[test]
    fn build_event_omits_none_target_id_and_error() {
        let ev = build_event(
            "abc123",
            TaskKind::Init,
            TaskPhase::Started,
            "repo-1",
            None,
            TaskOrigin::Manual,
            None,
            None,
        );
        let json = serde_json::to_value(&ev).unwrap();
        assert!(json.get("targetId").is_none());
        assert!(json.get("progress").is_none());
        assert!(json.get("error").is_none());
    }
}
