import type { BackupWarning } from "../lib/types";

interface Props {
  warnings: BackupWarning[];
  /** Total issues reported — can exceed `warnings.length` (the stored list is capped). */
  warningCount: number;
  missingSourceCount: number;
  /** Show the "retention was skipped" note (live runs only; history doesn't record it). */
  retentionSkipped?: boolean;
  /** macOS only: at least one warning looks like a permission error and Full Disk Access is off. */
  fdaHint?: boolean;
}

/**
 * The detail view for a backup that saved its snapshot but couldn't read everything (restic
 * exit 3), shared by the backup modal and the Logs page. Missing source paths come first with
 * stronger wording than ordinary unreadable files, since they mean a whole source is absent
 * from the snapshot, not just a file or two. Mapped amber shades only — this text sits on
 * the page/box background, which flips between themes.
 */
export default function BackupWarningsList({
  warnings,
  warningCount,
  missingSourceCount,
  retentionSkipped,
  fdaHint,
}: Props) {
  const missing = warnings.filter((w) => w.missingSource);
  const unreadable = warnings.filter((w) => !w.missingSource);
  const hidden = Math.max(0, warningCount - warnings.length);

  return (
    <div className="space-y-2 text-xs">
      {missing.length > 0 && (
        <div className="space-y-1">
          <p className="font-medium text-amber-300">
            {missingSourceCount === 1 ? "Source path missing" : `${missingSourceCount} source paths missing`}
            {" "}— these were not backed up:
          </p>
          <ul className="font-mono text-amber-300 space-y-0.5">
            {missing.map((w, i) => (
              <li key={`m${i}`} className="break-all">{w.item}</li>
            ))}
          </ul>
        </div>
      )}

      {unreadable.length > 0 && (
        <div className="space-y-1">
          <p className="font-medium text-amber-300">Files that could not be read:</p>
          <ul className="max-h-48 overflow-auto font-mono text-amber-300 space-y-0.5">
            {unreadable.map((w, i) => (
              <li key={`u${i}`} className="break-all" title={w.message}>
                {w.item || w.message}
                {w.item && <span className="text-amber-400"> — {w.message}</span>}
              </li>
            ))}
          </ul>
        </div>
      )}

      {hidden > 0 && warnings.length > 0 && (
        <p className="text-amber-400">…and {hidden} more not shown</p>
      )}

      {warnings.length === 0 && warningCount > 0 && (
        <p className="text-amber-300">Details are not available for this run.</p>
      )}

      {retentionSkipped && (
        <p className="text-amber-300">
          Retention was skipped because a source path was missing. It will run after the next
          complete backup.
        </p>
      )}

      {fdaHint && (
        <p className="text-amber-300">
          Some paths require Full Disk Access. Go to System Settings → Privacy &amp; Security →
          Full Disk Access and add Resty Desktop.
        </p>
      )}
    </div>
  );
}
