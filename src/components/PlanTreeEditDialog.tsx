import { useEffect, useMemo, useRef, useState } from "react";
import { treeCountMatches, treePlanEdit } from "../lib/invoke";
import { baseName, type NodeVerdict, type SourceKind, type TreeRulesInput } from "../lib/planTree";
import { scopeOptions, type EditOutcome, type MatchPreview } from "../lib/planTreeEdit";
import { isWindows } from "../lib/utils";
import Button from "./Button";
import Modal from "./Modal";

export interface EditTarget {
  path: string;
  name: string;
  kind: SourceKind;
  verdict?: NodeVerdict;
}

interface Props {
  target: EditTarget | null;
  rules: TreeRulesInput;
  onClose: () => void;
  onApply: (outcome: EditOutcome) => void;
}

/** `exclude` picks a pattern (reached from the tree menu's "Exclude by pattern…"); `include` is
 * the confirmation of adding a folder or file as a source. */
type Mode = "exclude" | "include";

function modeFor(t: EditTarget): Mode {
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
 * The two dialogs the tree still needs; a plain click on a row's box needs neither. Excluding by
 * pattern (from the row's menu) offers the pattern scopes (this exact item, a folder's contents,
 * the name anywhere, …) with an editable pattern and a live "would leave out N items". Backing up
 * a folder or file that isn't under a source yet shows exactly which sources and patterns will be
 * added or replaced — computed and verified by the rule engine — or why it can't be done, when
 * the click would swallow existing sources or change the exclude list. Nothing is applied until
 * the user confirms.
 */
export default function PlanTreeEditDialog({ target, rules, onClose, onApply }: Props) {
  if (!target) return null;
  return <Body key={target.path} target={target} rules={rules} onClose={onClose} onApply={onApply} />;
}

function Body({ target, rules, onClose, onApply }: Omit<Props, "target"> & { target: EditTarget }) {
  const mode = modeFor(target);
  const windows = isWindows();
  const options = useMemo(
    // Sources from path-list files bound a scope like the plan's own: a pattern never reaches above one.
    () => scopeOptions(target.path, target.kind, [...rules.sources, ...(rules.listSources ?? [])], windows),
    [target, rules.sources, rules.listSources, windows],
  );
  const [optionId, setOptionId] = useState("exact");
  const [pattern, setPattern] = useState(options[0]?.pattern ?? "");
  const [preview, setPreview] = useState<MatchPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<EditOutcome | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const rulesRef = useRef(rules);
  rulesRef.current = rules;

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

  // Adding a source: compute the edit up front so the dialog can show it.
  useEffect(() => {
    if (mode === "exclude") return;
    setOutcome(null);
    setError(null);
    let cancelled = false;
    treePlanEdit(rulesRef.current, { kind: "include", path: target.path })
      .then((o) => !cancelled && setOutcome(o))
      .catch((e) => !cancelled && setError(String(e)));
    return () => {
      cancelled = true;
    };
  }, [mode, target.path]);

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

  const title = mode === "exclude" ? `Exclude “${target.name}”` : `Back up “${target.name}”`;
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

        {mode === "include" && !error && !outcome && <p className="text-xs text-gray-500">Working out the change…</p>}

        {mode === "include" && outcome && (
          <div className="text-xs text-gray-500 space-y-2">
            {outcome.addedSources.length > 0 && (
              <div>
                Adds as a backup source:
                <PatternList rules={outcome.addedSources} />
              </div>
            )}
            {outcome.removedSources.length > 0 && (
              <div>
                Replaces the sources it contains:
                <PatternList rules={outcome.removedSources} />
              </div>
            )}
            {outcome.addedRules.length > 0 && (
              <div>
                Adds {outcome.addedRules.length === 1 ? "this pattern" : "these patterns"} to the end of your exclude list:
                <PatternList rules={outcome.addedRules} />
              </div>
            )}
            {outcome.removedRules.length > 0 && (
              <div>
                Removes {outcome.removedRules.length === 1 ? "this pattern" : "these patterns"} from your exclude list:
                <PatternList rules={outcome.removedRules} />
              </div>
            )}
          </div>
        )}

        {error && <p className="text-sm text-amber-400 whitespace-pre-wrap break-words">{error}</p>}

        <div className="flex justify-end gap-2 pt-1">
          <Button variant="secondary" onClick={onClose}>Cancel</Button>
          <Button onClick={apply} loading={busy} disabled={!canApply}>
            {mode === "exclude" ? "Exclude" : "Back up"}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
