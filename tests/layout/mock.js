// Layout-test mock of the Tauri IPC surface (see run.mjs). Injected into the built app's index.html
// before it boots, so the real pages render against deliberately awkward fixtures: very long names
// (some with no spaces), many/long tags, error + warning history rows, a read-only repo.
// Unlisted commands resolve to null (and are reported as "unmocked"); add a handler when a page
// starts relying on a new command's result.
(() => {
  const LONG = (n, s) => s.repeat(Math.ceil(n / s.length)).slice(0, n);
  const REPO_NAME = "Offsite Backblaze B2 production archive for the finance and legal departments";
  const REPO_PATH = "b2:company-backups-production-eu-central-primary:/finance/legal/archives/2026/quarterly-statements-final";
  const repos = [
    { id: "repo-1", name: REPO_NAME, path: REPO_PATH, readOnly: false, deferPrune: false, lastPrunedAt: null },
    { id: "repo-2", name: "local", path: "/Users/someone/backups/local-repo", readOnly: true, deferPrune: false, lastPrunedAt: null },
    { id: "repo-3", name: LONG(110, "x"), path: LONG(150, "y"), readOnly: false, deferPrune: true, lastPrunedAt: 1700000000 },
  ];
  const HOST = "build-server-production-eu-central-primary-replica-01.internal.example-corp.com";
  const LONGTAG = "quarterly-finance-department-archive-2026-q1-final-approved";
  const tagSets = [
    [LONGTAG, "daily", "prod", "finance", "legal", "archive", "q1", "q2", "q3", "q4", "tag-a", "tag-b", "tag-c", "tag-d"],
    ["daily"], [], [LONG(90, "z")], ["a", "b", "c"],
  ];
  const snapshots = Array.from({ length: 14 }, (_, i) => ({
    id: ("abcdef0123456789".repeat(4)).slice(i, i + 64),
    short_id: ("abcdef0123456789").slice(i % 8, (i % 8) + 8),
    time: new Date(Date.UTC(2026, 0, 15 - i, 12, 0, 0)).toISOString(),
    hostname: i % 2 ? "laptop" : HOST,
    username: "someone",
    paths: i % 3 === 0
      ? ["/Users/someone/Documents/very/long/path/to/the/finance/department/shared/drive/archive/2026", "/etc", "/var/log"]
      : ["/home/someone"],
    tags: tagSets[i % tagSets.length],
    size: 1234567890 + i * 98765,
  }));
  snapshots.forEach((s, i) => { s.id = s.id.replace(/./g, (c, j) => "0123456789abcdef"[(i * 3 + j) % 16]); s.short_id = s.id.slice(0, 8); });
  const files = [
    ["quarterly_financial_statements_final_approved_by_board_2026_Q1_revision_17_signed.pdf", "file"],
    [LONG(170, "n") + ".bin", "file"],
    ["a folder with a rather long descriptive name that has spaces in it for readers", "dir"],
    ["notes.txt", "file"], ["photos", "dir"], ["link", "symlink"],
  ].map(([name, type], i) => ({
    name, path: "/Users/someone/" + name, type, size: type === "dir" ? undefined : 12345678 + i * 777,
    mtime: new Date(Date.UTC(2026, 0, 10 + i)).toISOString(), mode: 420,
  }));
  const longPlan = "Quarterly archive of the finance department shared drive and mail exports";
  const plans = [
    {
      id: "plan-1", name: longPlan, repoId: "repo-1",
      paths: ["/Users/someone/Documents/very/long/path/to/the/finance/department/shared/drive/archive/2026", "/etc", "/var/log"],
      tags: [LONGTAG, "daily"], excludes: ["*.tmp", "node_modules", "# comment", "/Users/someone/Library/Caches/com.example.very.long.cache.identifier"],
      excludeIfPresent: [".nobackup"], excludeCaches: true,
      retention: { keepLast: 5, keepDaily: 7, keepWeekly: 4, keepMonthly: 12, keepYearly: 3 },
      limitUpload: 1000, limitDownload: 2000,
      webhooks: [{ id: "w1", url: "https://hooks.example.com/services/T00000000/B00000000/" + LONG(60, "X"), provider: "slack", stages: { started: true, completed: true, failed: true } },
                 { id: "w2", url: "https://example.com/hook", provider: "custom", stages: { started: false, completed: true, failed: true }, template: "{plan} finished: {status} ({duration})" }],
      packSize: 32, excludeCloudFiles: false, useFsSnapshot: false, filesFrom: [], excludeFiles: [{ path: "/Users/someone/exclude-patterns-for-quarterly-archive.txt", ignoreCase: false }],
    },
    { id: "plan-2", name: "Docs", repoId: "repo-2", paths: ["/home/someone"], tags: [], excludes: [], excludeIfPresent: [], excludeCaches: false,
      excludeCloudFiles: false, useFsSnapshot: false, filesFrom: [], excludeFiles: [] },
    { id: "plan-3", name: LONG(120, "p"), repoId: "repo-3", paths: [LONG(200, "/seg")], tags: [], excludes: [], excludeIfPresent: [], excludeCaches: false,
      excludeCloudFiles: false, useFsSnapshot: false, filesFrom: [], excludeFiles: [] },
  ];
  const now = Math.floor(Date.now() / 1000);
  const schedules = [
    { id: "s1", name: "Nightly full backup of every workstation and the shared finance drive archive", planIds: ["plan-1", "plan-2", "plan-3"], cronExpr: "0 */6 * * 1-5", enabled: true, lastRunAt: now - 3600, nextRunAt: now - 60, createdAt: now - 99999 },
    { id: "s2", name: "short", planIds: ["plan-2"], cronExpr: "0 2 * * *", enabled: false, lastRunAt: undefined, nextRunAt: now + 7200, createdAt: now - 5000 },
  ];
  const history = Array.from({ length: 12 }, (_, i) => ({
    id: "h" + i, repoId: "repo-" + ((i % 3) + 1), repoName: i % 3 === 0 ? REPO_NAME : (i % 3 === 1 ? "local" : LONG(110, "x")),
    planId: "plan-" + ((i % 3) + 1), planName: i % 4 === 3 ? undefined : (i % 2 ? "Docs" : longPlan),
    snapshotId: i % 5 === 4 ? undefined : snapshots[i].id, startedAt: now - i * 86400, durationSeconds: 90 + i * 37.5,
    filesNew: 1234 * (i + 1), filesChanged: 56 * i, bytesAdded: 123456789 * (i + 1),
    error: i === 2 ? "repository is already locked exclusively by PID 12345 on build-server" : (i === 6 ? "Cancelled" : undefined),
    warningCount: i === 4 ? 3 : 0, missingSourceCount: i === 4 ? 1 : 0,
  }));
  const h = {
    is_app_setup: () => true,
    try_auto_unlock: () => ({ unlocked: true, reason: "" }),
    get_restic_version: () => "restic 0.17.3 compiled with go1.22.5 on darwin/arm64",
    file_portal_missing: () => false,
    get_tray_enabled: () => true, get_tray_warning: () => null, get_launch_at_login: () => true, get_launch_at_login_warning: () => null,
    get_auto_unlock: () => true, get_auto_unlock_supported: () => true, auto_unlock_needs_prompt_warning: () => null,
    get_auto_indexing: () => true, get_remote_auto_refresh: () => false,
    get_compression: () => "auto", get_restic_path: () => "/opt/homebrew/Cellar/restic/0.17.3/bin/restic", get_restore_path: () => "/Users/someone/Restored",
    get_db_size: () => ({ totalBytes: 123456789, reclaimableBytes: 45678901 }), check_full_disk_access: () => ({ supported: true, granted: false }),
    get_notification_settings: () => ({ started: true, successChanged: true, successUnchanged: false, failures: true, warnings: true }),
    get_default_retention: () => ({ enabled: true, policy: { keepLast: 10, keepDaily: 7, keepWeekly: 4, keepMonthly: 12, keepYearly: 99 } }),
    list_repos: () => repos, get_repo_password: () => "pw", get_repo_credentials: () => [["B2_ACCOUNT_ID", "id"], ["B2_ACCOUNT_KEY", "key"]],
    get_repo_stats: (a) => (a.repoId === "repo-2" ? null : { total_size: 456789012345, total_file_count: 1234567, snapshots_count: 1234, raw_size: 123456789012, cached_at: now - 7200 }),
    list_snapshots: () => snapshots, get_snapshot_index_status: () => Object.fromEntries(snapshots.map((s, i) => [s.id, i % 3 === 1 ? "pending" : "complete"])),
    get_index_progress: () => ({ cached: 5, total: 10 }), get_active_index_batch: () => null,
    list_files: () => files, search_snapshot_files: () => files, search_repo_files: () => files.map((f) => ({ ...f, snapshotId: snapshots[0].id, snapshotShortId: snapshots[0].short_id })),
    diff_snapshots: () => ({ entries: files.map((f, i) => ({ path: f.path, change: ["added", "removed", "modified"][i % 3] })), totalAdded: 2, totalRemoved: 2, totalModified: 2, truncated: false }),
    get_snapshot_stats: () => ({ totalSize: 123456789, totalFileCount: 4242 }),
    list_backup_plans: () => plans, list_schedules: () => schedules, list_backup_history: () => history,
    list_plan_last_runs: () => plans.map((p, i) => ({ planId: p.id, startedAt: now - 3600 * (i + 1), error: i === 2 ? "boom: something failed badly" : undefined, warningCount: i === 1 ? 2 : 0, missingSourceCount: 0 })),
    get_backup_warnings: () => [{ item: "/var/log/some/very/long/path/to/an/unreadable/file/that/restic/could/not/read.log", message: "open /var/log/...: permission denied", missingSource: false }],
    describe_cron_expr: () => "At minute 0 past every 6th hour on every day-of-week from Monday through Friday",
    preview_webhook: () => ({ started: "started", completed: "done", failed: "failed", unknownPlaceholders: [] }),
    "plugin:app|version": () => "0.3.0", "plugin:event|listen": () => 1, "plugin:event|unlisten": () => null,
  };
  let cb = 0;
  window.__TAURI_INTERNALS__ = {
    invoke: async (cmd, args) => { const f = h[cmd]; if (f) return f(args || {}); window.__unmocked = (window.__unmocked || []).concat(cmd); return null; },
    transformCallback: () => ++cb, unregisterCallback: () => {}, convertFileSrc: (p) => p,
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
  window.__MOCK_DATA__ = { snapshots, plans, schedules, repos };
})();
