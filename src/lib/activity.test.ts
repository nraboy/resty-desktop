import { describe, it, expect } from "vitest";
import { reduceStatsOps, initialStatsOpsState, reduceIndexBatches, type StatsOpsState } from "./activity";
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
