import { describe, it, expect } from "vitest";
import { reduceStatsOps, initialStatsOpsState, reduceIndexBatches, reduceSchedulerBackup, reducePrune, type StatsOpsState } from "./activity";
import type { TaskEvent } from "./types";

function taskEvent(overrides: Partial<TaskEvent>): TaskEvent {
  return {
    operationId: "op1",
    kind: "stats",
    phase: "started",
    repoId: "repoA",
    origin: "manual",
    at: 0,
    ...overrides,
  };
}

describe("reduceStatsOps", () => {
  it("adds an operation on started", () => {
    const state = reduceStatsOps(initialStatsOpsState, taskEvent({ operationId: "op1", repoId: "repoA", phase: "started" }));
    expect(state.inFlight.get("op1")).toBe("repoA");
  });

  it("removes the operation on finished and clears any failure marker", () => {
    let state = reduceStatsOps(initialStatsOpsState, taskEvent({ operationId: "op1", repoId: "repoA", phase: "started" }));
    state = reduceStatsOps(state, taskEvent({ operationId: "op1", repoId: "repoA", phase: "failed" }));
    expect(state.failed.has("repoA")).toBe(true);

    // A later successful attempt clears the prior failure marker.
    state = reduceStatsOps(state, taskEvent({ operationId: "op2", repoId: "repoA", phase: "started" }));
    state = reduceStatsOps(state, taskEvent({ operationId: "op2", repoId: "repoA", phase: "finished" }));
    expect(state.inFlight.has("op2")).toBe(false);
    expect(state.failed.has("repoA")).toBe(false);
  });

  it("removes the operation and sets the failure marker on failed", () => {
    let state = reduceStatsOps(initialStatsOpsState, taskEvent({ operationId: "op1", repoId: "repoA", phase: "started" }));
    state = reduceStatsOps(state, taskEvent({ operationId: "op1", repoId: "repoA", phase: "failed" }));
    expect(state.inFlight.has("op1")).toBe(false);
    expect(state.failed.has("repoA")).toBe(true);
  });

  it("a new attempt starting clears the prior failure marker even before it resolves", () => {
    let state = reduceStatsOps(initialStatsOpsState, taskEvent({ operationId: "op1", repoId: "repoA", phase: "started" }));
    state = reduceStatsOps(state, taskEvent({ operationId: "op1", repoId: "repoA", phase: "failed" }));
    expect(state.failed.has("repoA")).toBe(true);

    state = reduceStatsOps(state, taskEvent({ operationId: "op2", repoId: "repoA", phase: "started" }));
    expect(state.failed.has("repoA")).toBe(false);
  });

  it("removes the operation on cancelled without touching the failure marker", () => {
    const state = reduceStatsOps(initialStatsOpsState, taskEvent({ operationId: "op1", repoId: "repoA", phase: "started" }));
    const cancelled = reduceStatsOps(state, taskEvent({ operationId: "op1", repoId: "repoA", phase: "cancelled" }));
    expect(cancelled.inFlight.has("op1")).toBe(false);
    expect(cancelled.failed.has("repoA")).toBe(false);
  });

  it("ignores non-stats task kinds", () => {
    const state = reduceStatsOps(initialStatsOpsState, taskEvent({ kind: "backup", phase: "started" }));
    expect(state.inFlight.size).toBe(0);
    expect(state.failed.size).toBe(0);
  });

  it("is a no-op removing an unknown operationId", () => {
    const state = reduceStatsOps(initialStatsOpsState, taskEvent({ operationId: "op1", repoId: "repoA", phase: "started" }));
    const result = reduceStatsOps(state, taskEvent({ operationId: "unknown", phase: "finished" }));
    expect(result.inFlight.get("op1")).toBe("repoA");
    expect(result.inFlight.size).toBe(1);
  });

  it("dedups two concurrent operations against the same repo", () => {
    let state: StatsOpsState = reduceStatsOps(initialStatsOpsState, taskEvent({ operationId: "op1", repoId: "repoA", phase: "started" }));
    state = reduceStatsOps(state, taskEvent({ operationId: "op2", repoId: "repoA", phase: "started" }));
    expect([...new Set(state.inFlight.values())]).toEqual(["repoA"]);

    // Finishing op1 alone must not clear repoA from the derived set — op2 is still running.
    state = reduceStatsOps(state, taskEvent({ operationId: "op1", repoId: "repoA", phase: "finished" }));
    expect([...new Set(state.inFlight.values())]).toEqual(["repoA"]);

    state = reduceStatsOps(state, taskEvent({ operationId: "op2", repoId: "repoA", phase: "finished" }));
    expect(state.inFlight.size).toBe(0);
  });

  it("tracks two different repos independently", () => {
    let state: StatsOpsState = reduceStatsOps(initialStatsOpsState, taskEvent({ operationId: "op1", repoId: "repoA", phase: "started" }));
    state = reduceStatsOps(state, taskEvent({ operationId: "op2", repoId: "repoB", phase: "started" }));
    expect(new Set(state.inFlight.values())).toEqual(new Set(["repoA", "repoB"]));

    state = reduceStatsOps(state, taskEvent({ operationId: "op1", repoId: "repoA", phase: "finished" }));
    expect([...state.inFlight.values()]).toEqual(["repoB"]);
  });
});

describe("reduceIndexBatches", () => {
  const empty = new Map();

  it("starts a batch with zeroed progress and status running", () => {
    const state = reduceIndexBatches(empty, taskEvent({ kind: "index", operationId: "batch1", repoId: "repoA", phase: "started" }));
    expect(state.get("batch1")).toEqual({ operationId: "batch1", repoId: "repoA", itemsDone: 0, itemsTotal: 0, status: "running" });
  });

  it("updates itemsDone/itemsTotal from a matching progress event", () => {
    let state = reduceIndexBatches(empty, taskEvent({ kind: "index", operationId: "batch1", repoId: "repoA", phase: "started" }));
    state = reduceIndexBatches(state, taskEvent({
      kind: "index", operationId: "batch1", repoId: "repoA", phase: "progress",
      progress: { itemsDone: 3, itemsTotal: 10 },
    }));
    expect(state.get("batch1")).toEqual({ operationId: "batch1", repoId: "repoA", itemsDone: 3, itemsTotal: 10, status: "running" });
  });

  it("queues a batch with status queued on a pending event", () => {
    const state = reduceIndexBatches(empty, taskEvent({ kind: "index", operationId: "batch1", repoId: "repoA", phase: "pending" }));
    expect(state.get("batch1")).toEqual({ operationId: "batch1", repoId: "repoA", itemsDone: 0, itemsTotal: 0, status: "queued" });
  });

  it("promotes a queued batch to running on started", () => {
    let state = reduceIndexBatches(empty, taskEvent({ kind: "index", operationId: "batch1", repoId: "repoA", phase: "pending" }));
    state = reduceIndexBatches(state, taskEvent({ kind: "index", operationId: "batch1", repoId: "repoA", phase: "started" }));
    expect(state.get("batch1")).toEqual({ operationId: "batch1", repoId: "repoA", itemsDone: 0, itemsTotal: 0, status: "running" });
  });

  it("cancelling a still-queued batch removes it without ever seeing started", () => {
    let state = reduceIndexBatches(empty, taskEvent({ kind: "index", operationId: "batch1", repoId: "repoA", phase: "pending" }));
    state = reduceIndexBatches(state, taskEvent({ kind: "index", operationId: "batch1", repoId: "repoA", phase: "cancelled" }));
    expect(state.has("batch1")).toBe(false);
  });

  it("clears the matching operationId on finished/failed/cancelled", () => {
    for (const phase of ["finished", "failed", "cancelled"] as const) {
      const started = reduceIndexBatches(empty, taskEvent({ kind: "index", operationId: "batch1", repoId: "repoA", phase: "started" }));
      const cleared = reduceIndexBatches(started, taskEvent({ kind: "index", operationId: "batch1", repoId: "repoA", phase }));
      expect(cleared.has("batch1")).toBe(false);
    }
  });

  it("ignores a per-snapshot index event (targetId set)", () => {
    const state = reduceIndexBatches(empty, taskEvent({
      kind: "index", operationId: "op1", repoId: "repoA", phase: "finished", targetId: "snap1",
    }));
    expect(state.size).toBe(0);
  });

  it("a running batch is untouched by a concurrent per-snapshot event", () => {
    const started = reduceIndexBatches(empty, taskEvent({ kind: "index", operationId: "batch1", repoId: "repoA", phase: "started" }));
    const result = reduceIndexBatches(started, taskEvent({
      kind: "index", operationId: "op1", repoId: "repoA", phase: "finished", targetId: "snap1",
    }));
    expect(result).toBe(started);
  });

  it("ignores the background auto-indexer's index events", () => {
    const state = reduceIndexBatches(empty, taskEvent({
      kind: "index", operationId: "op1", repoId: "repoA", phase: "started", origin: "background", targetId: "snap1",
    }));
    expect(state.size).toBe(0);
  });

  it("ignores non-index task kinds", () => {
    const state = reduceIndexBatches(empty, taskEvent({ kind: "stats", phase: "started" }));
    expect(state.size).toBe(0);
  });

  it("a terminal event for an unknown operationId is a no-op", () => {
    const started = reduceIndexBatches(empty, taskEvent({ kind: "index", operationId: "batch2", repoId: "repoA", phase: "started" }));
    const result = reduceIndexBatches(started, taskEvent({ kind: "index", operationId: "batch1", repoId: "repoA", phase: "finished" }));
    expect(result).toBe(started);
  });

  it("a progress event for an unknown operationId is a no-op", () => {
    const started = reduceIndexBatches(empty, taskEvent({ kind: "index", operationId: "batch2", repoId: "repoA", phase: "started" }));
    const result = reduceIndexBatches(started, taskEvent({
      kind: "index", operationId: "batch1", repoId: "repoA", phase: "progress",
      progress: { itemsDone: 5, itemsTotal: 10 },
    }));
    expect(result).toBe(started);
  });

  it("tracks two concurrent batches (different repos) independently", () => {
    let state = reduceIndexBatches(empty, taskEvent({ kind: "index", operationId: "batchA", repoId: "repoA", phase: "started" }));
    state = reduceIndexBatches(state, taskEvent({ kind: "index", operationId: "batchB", repoId: "repoB", phase: "started" }));
    expect(state.size).toBe(2);

    state = reduceIndexBatches(state, taskEvent({
      kind: "index", operationId: "batchA", repoId: "repoA", phase: "progress",
      progress: { itemsDone: 4, itemsTotal: 9 },
    }));
    // Updating batchA's progress must not touch batchB's entry.
    expect(state.get("batchA")).toEqual({ operationId: "batchA", repoId: "repoA", itemsDone: 4, itemsTotal: 9, status: "running" });
    expect(state.get("batchB")).toEqual({ operationId: "batchB", repoId: "repoB", itemsDone: 0, itemsTotal: 0, status: "running" });

    // Finishing batchA alone must not remove batchB.
    state = reduceIndexBatches(state, taskEvent({ kind: "index", operationId: "batchA", repoId: "repoA", phase: "finished" }));
    expect(state.has("batchA")).toBe(false);
    expect(state.has("batchB")).toBe(true);
  });
});

describe("reduceSchedulerBackup", () => {
  function schedulerEvent(overrides: Partial<TaskEvent>): TaskEvent {
    return taskEvent({ origin: "scheduler", kind: "backup", repoId: "repoA", targetId: "planA", ...overrides });
  }

  it("starts a run on a scheduler backup 'started' event", () => {
    const state = reduceSchedulerBackup(null, schedulerEvent({ operationId: "opBackup1", phase: "started" }));
    expect(state).toEqual({
      runId: "opBackup1",
      currentOperationId: "opBackup1",
      planId: "planA",
      planName: null,
      progress: null,
      phase: "backup",
      backupFinished: false,
    });
  });

  it("ignores a manual-origin backup event", () => {
    const state = reduceSchedulerBackup(null, schedulerEvent({ origin: "manual", operationId: "opBackup1", phase: "started" }));
    expect(state).toBeNull();
  });

  it("updates progress from a matching backup progress event", () => {
    let state = reduceSchedulerBackup(null, schedulerEvent({ operationId: "opBackup1", phase: "started" }));
    state = reduceSchedulerBackup(state, schedulerEvent({
      operationId: "opBackup1", phase: "progress",
      progress: { percentDone: 0.5, itemsDone: 5, itemsTotal: 10 },
    }));
    expect(state?.progress).toEqual({ percentDone: 0.5, itemsDone: 5, itemsTotal: 10 });
  });

  it("ignores a progress event for a stale operationId", () => {
    const started = reduceSchedulerBackup(null, schedulerEvent({ operationId: "opBackup1", phase: "started" }));
    const result = reduceSchedulerBackup(started, schedulerEvent({ operationId: "opOther", phase: "progress" }));
    expect(result).toBe(started);
  });

  it("stays visible with backupFinished set once the backup op finishes", () => {
    let state = reduceSchedulerBackup(null, schedulerEvent({ operationId: "opBackup1", phase: "started" }));
    state = reduceSchedulerBackup(state, schedulerEvent({ operationId: "opBackup1", phase: "finished" }));
    expect(state).toMatchObject({ phase: "backup", backupFinished: true, currentOperationId: "opBackup1" });
  });

  it("clears on a matching backup failed/cancelled event", () => {
    for (const phase of ["failed", "cancelled"] as const) {
      const started = reduceSchedulerBackup(null, schedulerEvent({ operationId: "opBackup1", phase: "started" }));
      const result = reduceSchedulerBackup(started, schedulerEvent({ operationId: "opBackup1", phase }));
      expect(result).toBeNull();
    }
  });

  it("transitions to retention on a matching forget 'started' event once the backup has finished", () => {
    let state = reduceSchedulerBackup(null, schedulerEvent({ operationId: "opBackup1", phase: "started" }));
    state = reduceSchedulerBackup(state, schedulerEvent({ operationId: "opBackup1", phase: "finished" }));
    state = reduceSchedulerBackup(state, schedulerEvent({ kind: "forget", operationId: "opForget1", phase: "started" }));
    expect(state).toMatchObject({ phase: "retention", currentOperationId: "opForget1", runId: "opBackup1" });
  });

  it("ignores a forget 'started' event before the backup has finished", () => {
    const started = reduceSchedulerBackup(null, schedulerEvent({ operationId: "opBackup1", phase: "started" }));
    const result = reduceSchedulerBackup(started, schedulerEvent({ kind: "forget", operationId: "opForget1", phase: "started" }));
    expect(result).toBe(started);
  });

  it("ignores a forget 'started' event for a different plan", () => {
    let state = reduceSchedulerBackup(null, schedulerEvent({ operationId: "opBackup1", phase: "started" }));
    state = reduceSchedulerBackup(state, schedulerEvent({ operationId: "opBackup1", phase: "finished" }));
    const result = reduceSchedulerBackup(state, schedulerEvent({ kind: "forget", operationId: "opForget1", phase: "started", targetId: "planB" }));
    expect(result).toBe(state);
  });

  it("clears on a matching forget finished/failed event", () => {
    for (const phase of ["finished", "failed"] as const) {
      let state = reduceSchedulerBackup(null, schedulerEvent({ operationId: "opBackup1", phase: "started" }));
      state = reduceSchedulerBackup(state, schedulerEvent({ operationId: "opBackup1", phase: "finished" }));
      state = reduceSchedulerBackup(state, schedulerEvent({ kind: "forget", operationId: "opForget1", phase: "started" }));
      state = reduceSchedulerBackup(state, schedulerEvent({ kind: "forget", operationId: "opForget1", phase }));
      expect(state).toBeNull();
    }
  });

  it("a new plan's 'started' event displaces the previous plan's lingering finished state", () => {
    let state = reduceSchedulerBackup(null, schedulerEvent({ operationId: "opBackup1", phase: "started", targetId: "planA" }));
    state = reduceSchedulerBackup(state, schedulerEvent({ operationId: "opBackup1", phase: "finished", targetId: "planA" }));
    state = reduceSchedulerBackup(state, schedulerEvent({ operationId: "opBackup2", phase: "started", targetId: "planB" }));
    expect(state).toEqual({
      runId: "opBackup2",
      currentOperationId: "opBackup2",
      planId: "planB",
      planName: null,
      progress: null,
      phase: "backup",
      backupFinished: false,
    });
  });

  it("ignores non-backup/forget scheduler task kinds", () => {
    const result = reduceSchedulerBackup(null, schedulerEvent({ kind: "stats", phase: "started" }));
    expect(result).toBeNull();
  });
});

describe("reducePrune", () => {
  it("starts a row with zeroed progress and no repo label", () => {
    const state = reducePrune(null, taskEvent({ kind: "prune", operationId: "op1", phase: "started" }));
    expect(state).toEqual({ operationId: "op1", itemsDone: 0, itemsTotal: 0, repoLabel: null });
  });

  it("updates itemsDone/itemsTotal/repoLabel on a matching progress event", () => {
    let state = reducePrune(null, taskEvent({ kind: "prune", operationId: "op1", phase: "started" }));
    state = reducePrune(state, taskEvent({
      kind: "prune", operationId: "op1", phase: "progress",
      progress: { itemsDone: 2, itemsTotal: 5, label: "My Repo" },
    }));
    expect(state).toEqual({ operationId: "op1", itemsDone: 2, itemsTotal: 5, repoLabel: "My Repo" });
  });

  it("ignores a progress event for a different (stale) operationId", () => {
    const state = reducePrune(null, taskEvent({ kind: "prune", operationId: "op1", phase: "started" }));
    const result = reducePrune(state, taskEvent({
      kind: "prune", operationId: "opStale", phase: "progress",
      progress: { itemsDone: 9, itemsTotal: 9, label: "Other Repo" },
    }));
    expect(result).toBe(state);
  });

  it.each(["finished", "failed", "cancelled"] as const)("clears on a matching %s event", (phase) => {
    const state = reducePrune(null, taskEvent({ kind: "prune", operationId: "op1", phase: "started" }));
    const result = reducePrune(state, taskEvent({ kind: "prune", operationId: "op1", phase }));
    expect(result).toBeNull();
  });

  it("ignores a terminal event for a different (stale) operationId", () => {
    const state = reducePrune(null, taskEvent({ kind: "prune", operationId: "op1", phase: "started" }));
    const result = reducePrune(state, taskEvent({ kind: "prune", operationId: "opStale", phase: "finished" }));
    expect(result).toBe(state);
  });

  it("ignores non-prune task kinds", () => {
    const result = reducePrune(null, taskEvent({ kind: "backup", phase: "started" }));
    expect(result).toBeNull();
  });

  it("a single-repo prune (no progress events) still starts and clears cleanly", () => {
    let state = reducePrune(null, taskEvent({ kind: "prune", operationId: "op1", phase: "started" }));
    expect(state).toEqual({ operationId: "op1", itemsDone: 0, itemsTotal: 0, repoLabel: null });
    state = reducePrune(state, taskEvent({ kind: "prune", operationId: "op1", phase: "finished" }));
    expect(state).toBeNull();
  });
});
