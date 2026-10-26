import { useEffect, useMemo, useRef, useState } from "react";
import { treeCountMatches, treePlanEdit } from "../lib/invoke";
import { baseName, type NodeVerdict, type SourceKind, type TreeRulesInput } from "../lib/planTree";
import { escapeGlob, scopeOptions, type EditOutcome, type MatchPreview } from "../lib/planTreeEdit";
import { isWindows } from "../lib/utils";
import Button from "./Button";
import Modal from "./Modal";

export interface EditTarget {
  path: string;
  name: string;
  kind: SourceKind;
  verdict?: NodeVerdict;
  /** The path is one of the plan's source folders/files. */
  isSource: boolean;
}

interface Props {
  target: EditTarget | null;
  rules: TreeRulesInput;
  onClose: () => void;
  onApply: (outcome: EditOutcome) => void;
}

type Mode = "removeSource" | "exclude" | "include";

function modeFor(t: EditTarget): Mode {
  if (t.isSource) return "removeSource";
  return t.verdict?.included ? "exclude" : "include";
}


function PatternList({ rules }: { rules: string[] }) {
  return (
    <ul className="mt-1 space-y-0.5">
      {rules.map((r, i) => (
        <li key={i} className="text-xs font-mono text-gray-300 break-all">{r}</li>
      ))}
    </ul>
  );
}

/**
 * The dialog behind a click on a row's box. Excluding offers the pattern scopes (this exact item,
 * a folder's contents, the name anywhere, …) with an editable pattern and a live "would leave out
 * N items"; including shows exactly which sources/patterns will be added — computed and verified
 * by the rule engine — or why it can't be done. Nothing is applied until the user confirms.
 */
export default function PlanTreeEditDialog({ target, rules, onClose, onApply }: Props) {
  if (!target) return null;
  return <Body key={target.path} target={target} rules={rules} onClose={onClose} onApply={onApply} />;
}

function Body({ target, rules, onClose, onApply }: Omit<Props, "target"> & { target: EditTarget }) {
  const mode = modeFor(target);
  const windows = isWindows();
  const options = useMemo(
    () => scopeOptions(target.path, target.kind, rules.sources, windows),
    [target, rules.sources, windows],
  );
  const [optionId, setOptionId] = useState("exact");
  const [pattern, setPattern] = useState(options[0]?.pattern ?? "");
  const [preview, setPreview] = useState<MatchPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<EditOutcome | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [includeScope, setIncludeScope] = useState<"item" | "folder">("item");
  const rulesRef = useRef(rules);
  rulesRef.current = rules;

  const ancestorExcluded = !!target.verdict?.at && target.verdict.at !== target.path;
  const ancestor = target.verdict?.at ?? "";

  // Excluding: a live count of what the pattern would leave out.
  useEffect(() => {
    if (mode !== "exclude") return;
    setPreview(null);
    setPreviewError(null);
    const t = setTimeout(() => {
      treeCountMatches(rulesRef.current, pattern, 2000)
        .then(setPreview)
        .catch((e) => setPreviewError(String(e)));
    }, 300);
    return () => clearTimeout(t);
  }, [mode, pattern]);

  // Including / removing a source: compute the edit up front so the dialog can show it.
  useEffect(() => {
    if (mode === "exclude") return;
    setOutcome(null);
    setError(null);
    let cancelled = false;
    const action =
      mode === "removeSource"
        ? ({ kind: "exclude", path: target.path, pattern: escapeGlob(target.path, windows) } as const)
        : ({ kind: "include", path: target.path, scope: includeScope } as const);
    treePlanEdit(rulesRef.current, action)
      .then((o) => !cancelled && setOutcome(o))
      .catch((e) => !cancelled && setError(String(e)));
    return () => {
      cancelled = true;
    };
  }, [mode, target.path, includeScope, windows]);

  const apply = async () => {
    setBusy(true);
    setError(null);
    try {
      const o =
        mode === "exclude"
          ? await treePlanEdit(rules, { kind: "exclude", path: target.path, pattern })
          : outcome;
      if (!o) return;
      onApply(o);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // An item outside every source being added (it holds existing sources, so the click asked first).
  const addingSource = mode === "include" && target.verdict?.reason?.kind === "outsideSources";
  const title =
    mode === "exclude"
      ? `Exclude “${target.name}”`
      : mode === "removeSource"
        ? "Stop backing up this item"
        : addingSource
          ? `Back up “${target.name}”`
          : `Include “${target.name}”`;
  const canApply =
    mode === "exclude" ? pattern.trim() !== "" && !busy : !!outcome && !error && !busy;

  return (
    <Modal title={title} open onClose={onClose} maxWidth="max-w-lg">
      <div className="space-y-4">
        <p className="text-xs font-mono text-gray-500 break-all">{target.path}</p>

        {mode === "exclude" && (
          <>
            <div role="radiogroup" aria-label="What to exclude" className="space-y-1.5">
              {options.map((o) => (
                <label key={o.id} className="flex items-start gap-2 text-sm text-gray-300">
                  <input
                    type="radio"
                    name="scope"
                    className="mt-1"
                    checked={optionId === o.id}
                    onChange={() => {
                      setOptionId(o.id);
                      setPattern(o.pattern);
                    }}
                  />
                  <span>{o.label}</span>
                </label>
              ))}
            </div>
            <div>
              <label className="block text-xs text-gray-500 mb-1" htmlFor="tree-pattern">Pattern (you can edit it)</label>
              <input
                id="tree-pattern"
                value={pattern}
                onChange={(e) => {
                  setPattern(e.target.value);
                  setOptionId("custom");
                }}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                className="w-full px-3 py-2 rounded-lg bg-gray-800 border border-gray-700 text-sm font-mono text-gray-300 focus:outline-none focus:border-blue-500"
              />
            </div>
            <p className="text-xs text-gray-500" aria-live="polite">
              {previewError ? (
                <span className="text-amber-400">{previewError}</span>
              ) : !preview ? (
                "Checking what this would leave out…"
              ) : preview.count === 0 ? (
                "Doesn't leave out anything that's backed up right now."
              ) : (
                <>
                  Would leave out {preview.capped ? "at least " : ""}
                  <span className="text-gray-300">{preview.count.toLocaleString()}</span>{" "}
                  {preview.count === 1 ? "item" : "items"}
                  {preview.sample.length > 0 && (
                    <>
                      {" "}— e.g. <span className="font-mono">{preview.sample.map(baseName).join(", ")}</span>
                    </>
                  )}
                  .
                </>
              )}
            </p>
          </>
        )}

        {mode === "include" && (
          <>
            {ancestorExcluded && (
              <div role="radiogroup" aria-label="What to include" className="space-y-1.5">
                <label className="flex items-start gap-2 text-sm text-gray-300">
                  <input type="radio" name="iscope" className="mt-1" checked={includeScope === "item"} onChange={() => setIncludeScope("item")} />
                  <span>
                    Only “{target.name}” — keep the rest of <span className="font-mono">{baseName(ancestor)}</span> excluded
                  </span>
                </label>
                <label className="flex items-start gap-2 text-sm text-gray-300">
                  <input type="radio" name="iscope" className="mt-1" checked={includeScope === "folder"} onChange={() => setIncludeScope("folder")} />
                  <span>
                    The whole folder <span className="font-mono">{baseName(ancestor)}</span> and everything in it
                  </span>
                </label>
              </div>
            )}
            {!error && !outcome && <p className="text-xs text-gray-500">Working out the change…</p>}
          </>
        )}

        {mode === "removeSource" && !error && !outcome && <p className="text-xs text-gray-500">Working out the change…</p>}
        {mode === "removeSource" && outcome && (
          <p className="text-sm text-gray-300">
            This folder or file stops being backed up. Nothing is deleted from disk or from existing snapshots.
          </p>
        )}

        {mode !== "exclude" && outcome && (
          <div className="text-xs text-gray-500 space-y-2">
            {outcome.addedSources.length > 0 && (
              <div>
                Adds as a backup source:
                <PatternList rules={outcome.addedSources} />
              </div>
            )}
            {outcome.removedSources.length > 0 && (
              <div>
                {mode === "removeSource" ? "Removes the source:" : "Replaces the sources it contains:"}
                <PatternList rules={outcome.removedSources} />
              </div>
            )}
            {outcome.addedRules.length > 0 && (
              <div>
                Adds {outcome.addedRules.length === 1 ? "this pattern" : "these patterns"} to the end of your exclude list:
                <PatternList rules={outcome.addedRules} />
              </div>
            )}
          </div>
        )}

        {error && <p className="text-sm text-amber-400 whitespace-pre-wrap break-words">{error}</p>}

        <div className="flex justify-end gap-2 pt-1">
          <Button variant="secondary" onClick={onClose}>Cancel</Button>
          <Button onClick={apply} loading={busy} disabled={!canApply}>
            {mode === "exclude" ? "Exclude" : mode === "removeSource" ? "Remove" : addingSource ? "Back up" : "Include"}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
