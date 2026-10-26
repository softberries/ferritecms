// SEO (07 §3 Editor; P28): the title and description search results and link previews
// use. Soft counters at 60 and 160 characters say where results often cut them off; the
// hard limits are the server's (200 and 500). Below, a search result preview, and "Reset
// to defaults" clears both (the site then uses the item's title and excerpt). The values
// are part of the one draft (`meta.seo`): they autosave, enter the 412 conflict list as
// "SEO", and a change to them alone is a full save (a new version). Revisions do not keep
// them (P13); the revisions drawer says so.

import { RotateCcwIcon } from "lucide-react";
import { useId, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "cn";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { uiLocale } from "@/lib/locale";
import type { EditorDocument } from "../editor-state";
import { SearchPreview } from "./search-preview";

/** Hard limits (the server's) and soft ones (where results are often cut off). */
const SEO_LIMITS = {
  title: { max: 200, soft: 60 },
  description: { max: 500, soft: 160 },
} as const;

const chars = (s: string) => [...s].length;

/** A server message as a sentence (core starts it in lower case). */
const sentence = (s: string | undefined) => (s ? s[0]!.toUpperCase() + s.slice(1) : undefined);

export function SeoSection({
  doc,
  siteUrl,
  siteName,
  disabled,
}: {
  doc: EditorDocument;
  siteUrl: string | null;
  siteName: string;
  disabled: boolean;
}) {
  const { t } = useTranslation("editor");
  const { seo } = doc.meta;
  const set = (patch: Partial<typeof seo>) => {
    if (patch.title !== undefined) doc.clearFieldError("seo.title");
    if (patch.description !== undefined) doc.clearFieldError("seo.description");
    doc.setMeta({ seo: { ...doc.meta.seo, ...patch } });
  };
  const permalink = doc.item?.permalink ?? null;
  const url = siteUrl && permalink ? new URL(permalink, siteUrl).toString() : null;
  const previewTitle = seo.title.trim() || doc.title.trim() || t("title.untitled");
  const previewDescription = seo.description.trim() || doc.meta.excerpt.trim();
  const empty = seo.title === "" && seo.description === "";
  return (
    <section aria-labelledby="panel-seo" data-slot="panel-seo" className="flex flex-col gap-4">
      <div className="flex items-center justify-between gap-2">
        <h3 id="panel-seo" className="text-sm font-medium">
          {t("seo.heading")}
        </h3>
        {empty || disabled ? null : (
          <Button
            type="button"
            variant="ghost"
            size="xs"
            className="-mr-1.5 text-muted-foreground"
            onClick={() => set({ title: "", description: "" })}
          >
            <RotateCcwIcon data-icon="inline-start" />
            {t("seo.reset")}
          </Button>
        )}
      </div>
      <SeoInput
        kind="title"
        value={seo.title}
        disabled={disabled}
        placeholder={doc.title.trim() || t("seo.titlePlaceholder")}
        error={sentence(doc.fieldErrors["seo.title"])}
        onChange={(title) => set({ title })}
      />
      <SeoInput
        kind="description"
        value={seo.description}
        disabled={disabled}
        placeholder={doc.meta.excerpt.trim() || t("seo.descriptionPlaceholder")}
        error={sentence(doc.fieldErrors["seo.description"])}
        onChange={(description) => set({ description })}
      />
      <div className="flex flex-col gap-1.5">
        <span className="text-xs font-medium text-muted-foreground">{t("seo.preview")}</span>
        <SearchPreview
          title={previewTitle}
          url={url}
          description={previewDescription}
          siteName={siteName}
        />
      </div>
    </section>
  );
}

function SeoInput({
  kind,
  value,
  disabled,
  placeholder,
  error,
  onChange,
}: {
  kind: "title" | "description";
  value: string;
  disabled: boolean;
  placeholder: string;
  error: string | undefined;
  onChange: (v: string) => void;
}) {
  const { t } = useTranslation("editor");
  const id = useId();
  const hintId = useId();
  const errorId = useId();
  const { max, soft } = SEO_LIMITS[kind];
  const n = chars(value);
  const over = n > soft;
  const nf = new Intl.NumberFormat(uiLocale());
  const common = {
    id,
    value,
    disabled,
    placeholder,
    maxLength: max,
    "aria-describedby": error ? `${hintId} ${errorId}` : hintId,
    ...(error ? { "aria-invalid": true } : {}),
    onChange: (e: { target: { value: string } }) => onChange(e.target.value),
  };
  let control: ReactNode;
  if (kind === "title") control = <Input {...common} data-slot="seo-title" className="text-sm" />;
  else {
    control = (
      <Textarea
        {...common}
        data-slot="seo-description"
        rows={3}
        className="min-h-20 resize-y text-sm"
      />
    );
  }
  return (
    <div className="flex flex-col gap-1.5">
      <Label htmlFor={id}>{kind === "title" ? t("seo.title") : t("seo.description")}</Label>
      {control}
      <p
        id={hintId}
        className="flex items-start justify-between gap-3 text-xs text-muted-foreground"
      >
        <span>{t("seo.soft", { n: soft })}</span>
        <span
          data-slot={`seo-${kind}-count`}
          className={cn("shrink-0 tabular-nums", over && "font-medium text-foreground")}
        >
          {t("seo.count", { n: nf.format(n), soft: nf.format(soft) })}
        </span>
      </p>
      {error ? (
        <p id={errorId} className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
    </div>
  );
}
