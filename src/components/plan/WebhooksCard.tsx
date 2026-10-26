import { useCallback, useEffect, useRef, useState } from "react";
import { previewWebhook, testWebhook } from "../../lib/invoke";
import type { PlanWebhook, WebhookPreview, WebhookProvider } from "../../lib/types";
import ActionButton from "../ActionButton";
import Button from "../Button";
import { PencilIcon, XIcon } from "../icons";
import Input from "../Input";
import Modal from "../Modal";
import Tooltip from "../Tooltip";
import { CARD, type FormProps } from "./types";

/** Pre-filled body for a new custom-provider webhook — pinned by a Rust test
 *  (webhook.rs's DEFAULT_TEMPLATE) so the two can't drift apart. */
const DEFAULT_WEBHOOK_TEMPLATE =
  '{"event": "{eventName}", "plan": "{planName}", "repo": "{repoName}", "durationSeconds": {durationSeconds}}';

/** Placeholders a custom template may reference — rendered by the backend, listed
 *  here only as the editing hint (interpolation itself is Rust-side). */
const WEBHOOK_PLACEHOLDERS = [
  "{eventName}",
  "{repoName}",
  "{planName}",
  "{startedAt}",
  "{durationSeconds}",
  "{filesNew}",
  "{filesChanged}",
  "{bytesAdded}",
  "{snapshotId}",
  "{errorMessage}",
  "{warningCount}",
  "{missingSourceCount}",
];

const WEBHOOK_PROVIDER_LABELS: Record<WebhookProvider, string> = {
  generic: "Generic JSON",
  discord: "Discord",
  slack: "Slack",
  teams: "Teams",
  custom: "Custom JSON",
};

/** Labels aligned with the Settings page's notification category wording. Webhook
 *  "Success" deliberately isn't split into changed/unchanged like Settings' two
 *  success categories are — the payload already carries filesNew/filesChanged for
 *  downstream filtering. Webhook "Failures" also deliberately excludes user-cancelled
 *  backups, unlike the Settings category of the same name — see the tooltips below. */
const WEBHOOK_STAGE_LABELS: Record<"started" | "completed" | "failed", string> = {
  started: "Backup started",
  completed: "Success",
  failed: "Failures",
};


/** Per-plan webhooks: a read-only list, with all configuration in an Add/Edit modal. */
export default function WebhooksCard({ form, update }: FormProps) {
  const webhooks = form.webhooks;
  const setWebhooks = useCallback(
    (fn: (prev: PlanWebhook[]) => PlanWebhook[]) => update((p) => ({ webhooks: fn(p.webhooks) })),
    [update],
  );
  // Add/Edit Webhook modal — all webhook configuration happens there; the card
  // itself is a read-only list (URL, provider, trigger summary, edit/delete).
  const [webhookModalOpen, setWebhookModalOpen] = useState(false);
  const [editingWebhookId, setEditingWebhookId] = useState<string | null>(null);
  const [draftUrl, setDraftUrl] = useState("");
  const [draftProvider, setDraftProvider] = useState<WebhookProvider>("generic");
  const [draftStages, setDraftStages] = useState({ started: false, completed: true, failed: true });
  const [draftTemplate, setDraftTemplate] = useState(DEFAULT_WEBHOOK_TEMPLATE);
  // One message slot for the whole modal — Save validation failures and Send Test
  // results share it, so they render in one style and a new message always
  // replaces (never stacks on) the previous one.
  const [webhookNotice, setWebhookNotice] = useState<{ ok: boolean; message: string } | null>(null);
  const [testingWebhook, setTestingWebhook] = useState(false);
  const [webhookPreview, setWebhookPreview] = useState<WebhookPreview | null>(null);
  const [webhookPreviewLoading, setWebhookPreviewLoading] = useState(false);
  const [webhookPreviewError, setWebhookPreviewError] = useState("");
  const previewSeqRef = useRef(0);

  const openAddWebhookModal = useCallback(() => {
    setEditingWebhookId(null);
    setDraftUrl("");
    setDraftProvider("generic");
    setDraftStages({ started: false, completed: true, failed: true });
    setDraftTemplate(DEFAULT_WEBHOOK_TEMPLATE);
    setWebhookNotice(null);
    setWebhookModalOpen(true);
  }, []);

  const openEditWebhookModal = useCallback((w: PlanWebhook) => {
    setEditingWebhookId(w.id);
    setDraftUrl(w.url);
    setDraftProvider(w.provider);
    setDraftStages({ ...w.stages });
    // A legacy preset row has no template — switching it to Custom in the modal
    // should start from the working default, not an empty textarea.
    setDraftTemplate(w.template ?? DEFAULT_WEBHOOK_TEMPLATE);
    setWebhookNotice(null);
    setWebhookModalOpen(true);
  }, []);

  const closeWebhookModal = useCallback(() => {
    setWebhookModalOpen(false);
    setEditingWebhookId(null);
  }, []);

  const removeWebhook = useCallback((id: string) => {
    setWebhooks((prev) => prev.filter((w) => w.id !== id));
    if (editingWebhookId === id) closeWebhookModal();
  }, [editingWebhookId, closeWebhookModal, setWebhooks]);

  const commitWebhook = useCallback(() => {
    const url = draftUrl.trim();
    if (!/^https?:\/\//.test(url)) {
      setWebhookNotice({ ok: false, message: "Webhook URL must start with http:// or https://." });
      return;
    }
    if (draftProvider === "custom" && !draftTemplate.trim()) {
      setWebhookNotice({ ok: false, message: "Custom JSON webhooks need a JSON body template." });
      return;
    }
    // The preview refetch is async — while it's in flight, webhookPreviewError still
    // holds the *previous* draft's verdict, so committing now could either pass a
    // just-broken template or block a just-fixed one. Wait for the fresh verdict.
    if (draftProvider === "custom" && webhookPreviewLoading) {
      setWebhookNotice({ ok: false, message: "Validating template — try saving again in a moment." });
      return;
    }
    // preview_webhook already holds the parse verdict for the current draft — a
    // custom template that doesn't render valid JSON can't be committed.
    if (draftProvider === "custom" && webhookPreviewError) {
      setWebhookNotice({ ok: false, message: webhookPreviewError });
      return;
    }
    if (editingWebhookId) {
      setWebhooks((prev) =>
        prev.map((w) =>
          w.id === editingWebhookId
            ? {
                ...w,
                url,
                provider: draftProvider,
                stages: { ...draftStages },
                template: draftProvider === "custom" ? draftTemplate : undefined,
              }
            : w,
        ),
      );
    } else {
      setWebhooks((prev) => [
        ...prev,
        {
          id: crypto.randomUUID(),
          url,
          provider: draftProvider,
          stages: { ...draftStages },
          ...(draftProvider === "custom" ? { template: draftTemplate } : {}),
        },
      ]);
    }
    closeWebhookModal();
  }, [draftUrl, draftProvider, draftStages, draftTemplate, editingWebhookId, webhookPreviewError, webhookPreviewLoading, closeWebhookModal, setWebhooks]);

  const toggleDraftStage = useCallback((stage: "started" | "completed" | "failed") => {
    setDraftStages((prev) => ({ ...prev, [stage]: !prev[stage] }));
    setWebhookNotice(null);
  }, []);

  const sendTestWebhook = useCallback(async () => {
    setTestingWebhook(true);
    setWebhookNotice(null);
    try {
      await testWebhook(draftUrl.trim(), draftProvider, draftProvider === "custom" ? draftTemplate : undefined);
      setWebhookNotice({ ok: true, message: "Webhook delivered — check the target service." });
    } catch (err: any) {
      setWebhookNotice({ ok: false, message: String(err) });
    } finally {
      setTestingWebhook(false);
    }
  }, [draftUrl, draftProvider, draftTemplate]);

  // The modal's payload preview — always rendered while the modal is open, re-fetched
  // whenever the draft's provider/template changes so it always matches what would fire.
  const previewTemplate = draftProvider === "custom" ? draftTemplate : undefined;
  useEffect(() => {
    if (!webhookModalOpen) {
      setWebhookPreview(null);
      setWebhookPreviewError("");
      return;
    }
    const seq = ++previewSeqRef.current;
    setWebhookPreviewLoading(true);
    previewWebhook(draftProvider, previewTemplate)
      .then((p) => {
        if (seq !== previewSeqRef.current) return;
        setWebhookPreview(p);
        setWebhookPreviewError("");
      })
      .catch((err: any) => {
        if (seq !== previewSeqRef.current) return;
        setWebhookPreview(null);
        setWebhookPreviewError(String(err));
      })
      .finally(() => {
        if (seq === previewSeqRef.current) setWebhookPreviewLoading(false);
      });
  }, [webhookModalOpen, draftProvider, previewTemplate]);

  return (
    <>
      <div className={CARD}>
        <div className="flex items-center justify-between mb-1">
          <h2 className="text-sm font-medium text-gray-300">Webhooks (optional)</h2>
          <Button variant="secondary" size="sm" onClick={openAddWebhookModal}>+ Add Webhook</Button>
        </div>

        {webhooks.length === 0 ? (
          <p className="text-sm text-gray-500 text-center py-3">
            No webhooks. Add a URL to be notified via Discord, Slack, or any HTTP endpoint.
          </p>
        ) : (
          <ul className="space-y-3 mt-2">
            {webhooks.map((w) => {
              const selectedStages = (["started", "completed", "failed"] as const).filter(
                (s) => w.stages[s],
              );
              return (
                <li
                  key={w.id}
                  // Same row shape as RepositoriesPage's repo cards, one nesting
                  // level down (the card itself is already gray-900).
                  className="flex items-center justify-between p-4 rounded-xl border bg-gray-800 border-gray-700 hover:border-gray-600 transition-colors"
                >
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <p className="text-sm font-medium text-gray-100 truncate">{w.url}</p>
                      <span className="text-[10px] uppercase tracking-wide font-medium px-1.5 py-0.5 rounded bg-gray-900 border border-gray-700 text-gray-400 flex-shrink-0">
                        {WEBHOOK_PROVIDER_LABELS[w.provider]}
                      </span>
                    </div>
                    {selectedStages.length > 0 ? (
                      <p className="text-xs text-gray-500 mt-0.5">
                        On {selectedStages.map((s) => WEBHOOK_STAGE_LABELS[s]).join(", ")}
                      </p>
                    ) : (
                      <p className="text-xs text-amber-400 mt-0.5">
                        No stages selected — never fires.
                      </p>
                    )}
                  </div>
                  <div className="flex items-center gap-2 flex-shrink-0 ml-3">
                    <ActionButton
                      label="Edit"
                      title="Edit webhook"
                      tone="blue"
                      onClick={() => openEditWebhookModal(w)}
                      icon={<PencilIcon className="w-4 h-4" />}
                    />
                    <ActionButton
                      label="Remove"
                      title="Remove webhook"
                      tone="red"
                      onClick={() => removeWebhook(w.id)}
                      icon={<XIcon className="w-4 h-4" />}
                    />
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>

      <Modal
        title={editingWebhookId ? "Edit Webhook" : "Add Webhook"}
        open={webhookModalOpen}
        onClose={closeWebhookModal}
        maxWidth="max-w-lg"
      >
        <div className="space-y-4">
          <Input
            label="Endpoint URL"
            placeholder="https://discord.com/api/webhooks/…"
            value={draftUrl}
            onChange={(e) => { setDraftUrl(e.target.value); setWebhookNotice(null); }}
          />

          <div>
            <label className="block text-xs font-medium text-gray-400 uppercase tracking-wider mb-2">
              Payload Format
            </label>
            <select
              value={draftProvider}
              onChange={(e) => { setDraftProvider(e.target.value as WebhookProvider); setWebhookNotice(null); }}
              className="w-full appearance-none bg-gray-800 border border-gray-700 rounded-md px-3 py-2 text-sm text-gray-100 focus:outline-none focus:ring-2 focus:ring-blue-500"
            >
              <option value="generic">Generic JSON</option>
              <option value="discord">Discord</option>
              <option value="slack">Slack</option>
              <option value="teams">Teams</option>
              <option value="custom">Custom JSON</option>
            </select>
          </div>

          <div>
            <label className="block text-xs font-medium text-gray-400 uppercase tracking-wider mb-2">
              Trigger Stages
            </label>
            <div className="flex flex-wrap gap-4">
              {(["started", "completed", "failed"] as const).map((stage) => (
                <label
                  key={stage}
                  className="flex items-center gap-1.5 text-xs text-gray-400"
                >
                  <input
                    type="checkbox"
                    checked={draftStages[stage]}
                    onChange={() => toggleDraftStage(stage)}
                    className="rounded bg-gray-700 border-gray-600"
                  />
                  {stage === "completed" || stage === "failed" ? (
                    <Tooltip
                      content={
                        stage === "completed"
                          ? "Fires for both changed and unchanged runs — unlike Settings' notifications, this isn't split into two categories. The payload includes file counts if you need to filter downstream."
                          : "Unlike Settings' Failures notification, this does not fire for a backup you cancelled yourself."
                      }
                    >
                      <span className="underline decoration-dotted underline-offset-2">
                        {WEBHOOK_STAGE_LABELS[stage]}
                      </span>
                    </Tooltip>
                  ) : (
                    WEBHOOK_STAGE_LABELS[stage]
                  )}
                </label>
              ))}
            </div>
          </div>

          {draftProvider === "custom" && (
            <div>
              <p className="text-xs text-gray-500 mb-1">
                JSON body sent on each trigger — placeholders:{" "}
                {WEBHOOK_PLACEHOLDERS.join(" ")}
              </p>
              <textarea
                value={draftTemplate}
                onChange={(e) => { setDraftTemplate(e.target.value); setWebhookNotice(null); }}
                rows={4}
                placeholder={DEFAULT_WEBHOOK_TEMPLATE}
                className="w-full bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 text-xs font-mono text-gray-200 placeholder-gray-500 focus:outline-none focus:border-blue-500 resize-y"
                spellCheck={false}
              />
            </div>
          )}

          <div>
            <p className="text-xs text-gray-500 mb-1">Request body preview</p>
            {webhookPreviewLoading && (
              <p className="text-xs text-gray-500">Rendering payload…</p>
            )}
            {!webhookPreviewLoading && webhookPreviewError && (
              <p className="text-xs text-red-300">{webhookPreviewError}</p>
            )}
            {!webhookPreviewLoading && webhookPreview && (
              <>
                {webhookPreview.unknownPlaceholders.length > 0 && (
                  <p className="text-xs text-amber-400 mb-2">
                    Unknown placeholders (sent literally):{" "}
                    {webhookPreview.unknownPlaceholders.join(" ")}
                  </p>
                )}
                {(["started", "completed", "failed"] as const)
                  .filter((stage) => draftStages[stage])
                  .map((stage) => (
                    <div key={stage} className="mb-2 last:mb-0">
                      <p className="text-xs text-gray-500 mb-1">{WEBHOOK_STAGE_LABELS[stage]}</p>
                      <pre className="text-xs font-mono bg-gray-950 border border-gray-700 rounded-lg p-3 overflow-x-auto text-gray-300 whitespace-pre-wrap">
                        {webhookPreview[stage]}
                      </pre>
                    </div>
                  ))}
                {!(draftStages.started || draftStages.completed || draftStages.failed) && (
                  <p className="text-xs text-gray-500">
                    No stages selected — this webhook never fires.
                  </p>
                )}
              </>
            )}
          </div>

          {webhookNotice && (
            <div
              className={`text-sm rounded-lg px-3 py-2 border ${
                webhookNotice.ok
                  ? "bg-green-900/40 text-green-300 border-green-700"
                  : "bg-red-900/40 text-red-300 border-red-700"
              }`}
            >
              {webhookNotice.message}
            </div>
          )}

          <div className="flex items-center justify-between">
            <Button
              variant="secondary"
              loading={testingWebhook}
              onClick={sendTestWebhook}
            >
              Send Test
            </Button>
            <div className="flex gap-2">
              <Button variant="secondary" onClick={closeWebhookModal}>Cancel</Button>
              <Button onClick={commitWebhook}>
                {editingWebhookId ? "Save Changes" : "Add Webhook"}
              </Button>
            </div>
          </div>
        </div>
      </Modal>

    </>
  );
}
