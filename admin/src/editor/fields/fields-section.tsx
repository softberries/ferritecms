// The editor's custom fields (07 §3 Editor): one input per field of the type's schema,
// in the side panel, or, on wide screens for a type with more than 4 fields, as a
// "Fields" card under the body (decided by count). A type without a schema shows none
// (P36). The values are part of the one draft (`doc.setField`): they autosave, enter the
// 412 conflict list as "fields" and are PATCHed only when changed.
//
// Review Focus 2: stored values the schema no longer has are listed ("Not in the type any
// more: <name>, removed when you save") and dropped from the next save; an invalid value
// shows its message at once, a required field left empty once a save was refused for
// it, and the server's `422` messages at their paths.

import { BracesIcon, TriangleAlertIcon } from "lucide-react";
import { useCallback, useMemo, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "cn";
import type { EditorDocument } from "../editor-state";
import { FieldInput } from "./field-input";
import { FieldsContext, type FieldsContextValue } from "./fields-context";
import { fieldIssues, issueMessage, staleKeys, toSave } from "./fields-model";

/** A server message as a sentence (core's start in lower case: "required"). */
const sentence = (s: string) => (s ? s[0]!.toUpperCase() + s.slice(1) : s);

export function FieldsSection({ doc, card = false }: { doc: EditorDocument; card?: boolean }) {
  const { t } = useTranslation("editor");
  const defs = useMemo(() => doc.fieldDefs ?? [], [doc.fieldDefs]);
  const fields = doc.meta.fields;
  const stale = staleKeys(defs, fields);
  const issues = useMemo(() => fieldIssues(defs, toSave(defs, fields)), [defs, fields]);
  const { fieldErrors, saveAttempted, clearFieldError } = doc;
  const errorFor = useCallback(
    (path: string) => {
      const server = fieldErrors[path];
      if (server !== undefined) return sentence(server);
      const issue = issues[path];
      if (!issue || (issue.code === "required" && !saveAttempted)) return undefined;
      return issueMessage(issue);
    },
    [fieldErrors, issues, saveAttempted],
  );
  const context = useMemo<FieldsContextValue>(
    () => ({
      errorFor,
      edited: clearFieldError,
      disabled: doc.readOnly !== null || doc.restoring,
    }),
    [errorFor, clearFieldError, doc.readOnly, doc.restoring],
  );
  // Messages no input shows (a value of the featured image, a `fields` refusal without a
  // path): listed at the top, so a refused save always says why.
  const inputs = defs.map((d) => `fields.${d.name}`);
  const shownAt = (path: string) =>
    inputs.some((p) => path === p || path.startsWith(`${p}[`) || path.startsWith(`${p}.`));
  const general = [
    ...Object.entries(fieldErrors)
      .filter(([p]) => p.startsWith("fields") && !shownAt(p))
      .map(([, m]) => sentence(m)),
    ...Object.entries(issues)
      .filter(([p]) => !shownAt(p) && !(p in fieldErrors))
      .map(([p, issue]) => `${p.replace(/^fields\./, "")}: ${issueMessage(issue)}`),
  ];

  const body = (
    <FieldsContext value={context}>
      <div className="flex flex-col gap-5">
        {general.length > 0 ? (
          <ul role="alert" data-slot="fields-general-errors" className="text-sm text-destructive">
            {general.map((m) => (
              <li key={m}>{m}</li>
            ))}
          </ul>
        ) : null}
        {stale.length > 0 ? <StaleNotice names={stale} /> : null}
        {defs.map((def) => (
          <FieldInput
            key={def.name}
            def={def}
            value={fields[def.name]}
            path={`fields.${def.name}`}
            onChange={(v) => doc.setField(def.name, v)}
          />
        ))}
      </div>
    </FieldsContext>
  );

  if (card) {
    return (
      <section
        aria-labelledby="editor-fields-card"
        data-slot="fields-card"
        className="mt-12 rounded-xl border bg-card p-5 shadow-1 sm:p-6"
      >
        <Heading id="editor-fields-card" count={defs.length} as="h2">
          {t("fields.heading")}
        </Heading>
        {body}
      </section>
    );
  }
  return (
    <section
      aria-labelledby="panel-fields"
      data-slot="panel-fields"
      className="flex flex-col gap-3"
    >
      <Heading id="panel-fields" count={defs.length} as="h3">
        {t("fields.heading")}
      </Heading>
      {body}
    </section>
  );
}

function Heading({
  id,
  count,
  as: Tag,
  children,
}: {
  id: string;
  count: number;
  as: "h2" | "h3";
  children: ReactNode;
}) {
  return (
    <div className={cn("flex items-center gap-2", Tag === "h2" && "mb-5")}>
      <BracesIcon aria-hidden="true" className="size-4 text-muted-foreground" />
      <Tag id={id} className={cn("font-medium", Tag === "h2" ? "text-base" : "text-sm")}>
        {children}
      </Tag>
      {count > 0 ? (
        <span aria-hidden="true" className="text-xs text-muted-foreground tabular-nums">
          {count}
        </span>
      ) : null}
    </div>
  );
}

/** The stored values the type no longer has: they go with the next save. */
function StaleNotice({ names }: { names: string[] }) {
  const { t } = useTranslation("editor");
  return (
    <div
      role="note"
      data-slot="stale-keys"
      className="flex gap-2.5 rounded-lg border border-warning/40 bg-warning/8 px-3 py-2.5 text-sm text-foreground"
    >
      <TriangleAlertIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-warning" />
      <div className="flex min-w-0 flex-col gap-1">
        <ul className="flex flex-col gap-0.5">
          {names.map((name) => (
            <li key={name} data-slot="stale-key" className="break-words">
              {t("fields.staleBefore")}
              <code className="font-mono text-xs font-medium">{name}</code>
              {t("fields.staleAfter")}
            </li>
          ))}
        </ul>
        <p className="text-xs text-muted-foreground">{t("fields.staleHint")}</p>
      </div>
    </div>
  );
}
