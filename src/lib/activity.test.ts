import { describe, it, expect } from "vitest";
import { reduceStatsOps, initialStatsOpsState, type StatsOpsState } from "./activity";
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
