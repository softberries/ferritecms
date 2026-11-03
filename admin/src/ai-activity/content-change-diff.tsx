// The diff of a content or media change (design 17 §11): title, excerpt and body through
// the revisions drawer's word diff, then a table of the other members that changed
// (status, slug, parent, terms, SEO, custom fields, alt text).

import { useTranslation } from "react-i18next";
import { diffRevision } from "@/editor/diff";
import { DiffLegend, RevisionDiff } from "@/editor/revisions/revision-diff";
import { changed } from "@/editor/revisions/revision-model";
import { JsonDiffTable } from "./json-diff";
import { contentFieldChanges } from "./model";

interface Side {
  title: string;
  excerpt: string;
  body: unknown;
}

function sideOf(v: unknown): Side {
  const o = (typeof v === "object" && v !== null ? v : {}) as Record<string, unknown>;
  return {
    title: typeof o.title === "string" ? o.title : "",
    excerpt: typeof o.excerpt === "string" ? o.excerpt : "",
    body: o.body ?? { type: "doc", content: [] },
  };
}

export function ContentChangeDiff({ before, after }: { before: unknown; after: unknown }) {
  const { t } = useTranslation("ai");
  const d = diffRevision(sideOf(before), sideOf(after));
  const fields = contentFieldChanges(before ?? undefined, after ?? undefined);
  const parts = [
    ["title", d.title],
    ["excerpt", d.excerpt],
    ["body", d.body],
  ] as const;
  const text = parts.filter(([, p]) => changed(p));
  return (
    <div data-slot="content-change-diff" className="flex min-w-0 flex-col gap-4">
      <DiffLegend />
      {text.map(([key, p]) => (
        <section key={key} className="flex min-w-0 flex-col gap-1.5">
          <h4 className="text-xs font-medium text-muted-foreground">{t(`activity.diff.${key}`)}</h4>
          <RevisionDiff parts={[...p]} className="text-sm" />
        </section>
      ))}
      {fields.length > 0 || text.length === 0 ? (
        <section className="flex min-w-0 flex-col gap-1.5">
          {text.length > 0 ? (
            <h4 className="text-xs font-medium text-muted-foreground">
              {t("activity.diff.fields")}
            </h4>
          ) : null}
          <JsonDiffTable changes={fields} />
        </section>
      ) : null}
    </div>
  );
}
