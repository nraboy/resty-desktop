import Button from "../Button";

interface Props {
  isNew: boolean;
  saving: boolean;
  /** Why the last save (or load, or delete) failed; empty for none. */
  error: string;
  onSave: () => void;
  onCancel: () => void;
  onDelete: () => void;
}

/**
 * The plan editor's footer, below both tabs: Delete (existing plans) on the far left, then the
 * last error, Cancel and Save. Always in view, so a failed save's message and the actions can't
 * scroll away. A long message is cut to one line; its full text is the hover title.
 */
export default function PlanFooter({ isNew, saving, error, onSave, onCancel, onDelete }: Props) {
  return (
    <div className="flex items-center gap-3 px-6 py-3 border-t border-gray-800 bg-gray-950 flex-shrink-0">
      {!isNew && (
        <Button variant="danger" onClick={onDelete} disabled={saving}>
          Delete
        </Button>
      )}
      <div className="flex-1" />
      {error && (
        <p role="alert" title={error} className="min-w-0 truncate text-sm text-red-300">
          {error}
        </p>
      )}
      <div className="flex items-center gap-2 flex-shrink-0">
        <Button variant="secondary" onClick={onCancel} disabled={saving}>
          Cancel
        </Button>
        <Button onClick={onSave} loading={saving}>
          {isNew ? "Create Plan" : "Save Changes"}
        </Button>
      </div>
    </div>
  );
}
