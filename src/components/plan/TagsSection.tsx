import Button from "../Button";
import Input from "../Input";
import TruncatedText from "../TruncatedText";
import { CARD, type FormProps } from "./types";

/** The Settings tab's Tags section (its title is the section heading). */
export default function TagsSection({ form, update }: FormProps) {
  const { tags, tagInput } = form;
  const addTag = () => {
    const t = tagInput.trim();
    if (t && !tags.includes(t)) update((p) => ({ tags: [...p.tags, t], tagInput: "" }));
  };
  return (
    <div className={CARD}>
      <div className="flex gap-2 mb-2">
        <Input
          placeholder="Add tag…"
          value={tagInput}
          onChange={(e) => update({ tagInput: e.target.value })}
          onKeyDown={(e) => e.key === "Enter" && addTag()}
          className="flex-1"
        />
        <Button variant="secondary" size="sm" onClick={addTag}>Add</Button>
      </div>
      {tags.length > 0 && (
        <div className="flex flex-wrap gap-1.5 mt-2">
          {tags.map((t) => (
            <span
              key={t}
              className="inline-flex max-w-full items-center gap-1 text-xs bg-blue-900/40 text-blue-300 border border-blue-700/50 px-2 py-0.5 rounded-full"
            >
              <TruncatedText as="span" className="truncate" text={t} />
              <button
                onClick={() => update((p) => ({ tags: p.tags.filter((x) => x !== t) }))}
                className="flex-shrink-0 text-blue-400 hover:text-gray-50 transition-colors"
                aria-label={`Remove tag ${t}`}
              >
                ×
              </button>
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
