// SEO (07 §3 Editor; P28, M3-2 P33): the title and description search results and link
// previews use, the canonical URL and the robots directives. Soft counters at 60 and 160
// characters say where results often cut the text off; the hard limits are the server's
// (200, 500 and 2048), counted as it counts them: code points of the text trimmed of
// Unicode White_Space (`trimSeo`, which keeps a BOM), so an over-long paste is cut at a
// code-point boundary instead of by a native `maxLength` (UTF-16 units). Each counter
// has a polite live region that speaks only when a limit is crossed, not per keystroke.
// A search result preview follows the text (with the canonical
// address once it is a full one, and a note while `noindex` is on); then the canonical
// URL (the item's own address as its placeholder) and the five directives as a labelled
// group of switches. "Reset to defaults" clears all four (the site then uses the item's
// title, excerpt and address, and no robots tag). The values are part of the one draft
// (`meta.seo`): they autosave, enter the 412 conflict list as "SEO", and a change to them
// alone is a full save (a new version). Revisions do not keep them (P13); the revisions
// drawer says so.

import { EyeOffIcon, RotateCcwIcon } from "lucide-react";
import { useCallback, useId, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "cn";
import type { SchemaRobotsDto } from "@/api/schema";
import { FormField, SwitchRow } from "@/components/kit/form-field";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { uiLocale } from "@/lib/locale";
import {
  EMPTY_SEO,
  type EditorDocument,
  MAX_CANONICAL,
  type SeoDraft,
  canonicalAccepted,
  limitSeoEdit,
  refuseSeoOverflow,
  seoLength,
  trimSeo,
} from "../editor-state";
import { sortedRobots } from "../conflict";
import { SearchPreview } from "./search-preview";

/** The directives in the order the panel lists them (the most used first). */
const ROBOTS: readonly SchemaRobotsDto[] = [
  "noindex",
  "nofollow",
  "noarchive",
  "nosnippet",
  "noimageindex",
];

/** Hard limits (the server's) and soft ones (where results are often cut off). */
const SEO_LIMITS = {
  title: { max: 200, soft: 60 },
  description: { max: 500, soft: 160 },
} as const;

/**
 * A ref for a field limited to `max` (as the server counts): typed text past the limit
 * is refused before it lands (`beforeinput`), so the caret stays put, as with a native
 * `maxLength`.
 */
function useSeoLimit(max: number) {
  return useCallback(
    (el: HTMLInputElement | HTMLTextAreaElement | null) => {
      if (!el) return;
      const refuse = refuseSeoOverflow(max);
      el.addEventListener("beforeinput", refuse);
      return () => el.removeEventListener("beforeinput", refuse);
    },
    [max],
  );
}

/**
 * A server message about `path` as a sentence: core starts it with the path
 * (`seo.canonical must be …`), which the field's label already says.
 */
function sentence(path: string, s: string | undefined): string | undefined {
  if (!s) return undefined;
  const text = s.startsWith(`${path} `) ? s.slice(path.length + 1) : s;
  return text[0]!.toUpperCase() + text.slice(1);
}

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
  const set = (patch: Partial<SeoDraft>) => {
    for (const key of ["title", "description", "canonical"] as const) {
      if (patch[key] !== undefined) doc.clearFieldError(`seo.${key}`);
    }
    doc.setMeta({ seo: { ...doc.meta.seo, ...patch } });
  };
  const permalink = doc.item?.permalink ?? null;
  const ownUrl = siteUrl && permalink ? new URL(permalink, siteUrl).toString() : null;
  const canonical = trimSeo(seo.canonical);
  // Search engines show the canonical address; a half-typed one is not shown.
  const url = canonical !== "" && canonicalAccepted(canonical) ? canonical : ownUrl;
  const previewTitle = trimSeo(seo.title) || doc.title.trim() || t("title.untitled");
  const previewDescription = trimSeo(seo.description) || doc.meta.excerpt.trim();
  const empty =
    seo.title === "" && seo.description === "" && seo.canonical === "" && seo.robots.length === 0;
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
            onClick={() => set(EMPTY_SEO)}
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
        error={sentence("seo.title", doc.fieldErrors["seo.title"])}
        onChange={(title) => set({ title })}
      />
      <SeoInput
        kind="description"
        value={seo.description}
        disabled={disabled}
        placeholder={doc.meta.excerpt.trim() || t("seo.descriptionPlaceholder")}
        error={sentence("seo.description", doc.fieldErrors["seo.description"])}
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
        {seo.robots.includes("noindex") ? (
          <p
            data-slot="seo-noindex-note"
            className="flex items-start gap-1.5 text-xs text-muted-foreground"
          >
            <EyeOffIcon aria-hidden="true" className="mt-px size-3.5 shrink-0 text-warning" />
            {t("seo.noindexNote")}
          </p>
        ) : null}
      </div>
      <CanonicalField
        value={seo.canonical}
        disabled={disabled}
        placeholder={ownUrl ?? t("seo.canonicalPlaceholder")}
        serverError={sentence("seo.canonical", doc.fieldErrors["seo.canonical"])}
        onChange={(value) => set({ canonical: value })}
      />
      <RobotsGroup value={seo.robots} disabled={disabled} onChange={(robots) => set({ robots })} />
    </section>
  );
}

/**
 * The canonical URL: the server's `422` message, else, once the field was left, this
 * screen's own check (the server's rules), shown under it.
 */
function CanonicalField({
  value,
  disabled,
  placeholder,
  serverError,
  onChange,
}: {
  value: string;
  disabled: boolean;
  placeholder: string;
  serverError: string | undefined;
  onChange: (v: string) => void;
}) {
  const { t } = useTranslation("editor");
  const [left, setLeft] = useState(false);
  const limitRef = useSeoLimit(MAX_CANONICAL);
  const error =
    serverError ?? (left && !canonicalAccepted(value) ? t("seo.canonicalInvalid") : undefined);
  return (
    <FormField
      slot="seo-canonical-field"
      label={t("seo.canonical")}
      hint={t("seo.canonicalHint")}
      error={error}
    >
      {(control) => (
        <Input
          {...control}
          type="url"
          inputMode="url"
          autoComplete="off"
          spellCheck={false}
          data-slot="seo-canonical"
          className="text-sm"
          value={value}
          disabled={disabled}
          placeholder={placeholder}
          // A long address scrolls inside the field: the whole one on hover.
          title={value || undefined}
          onBlur={() => setLeft(true)}
          ref={limitRef}
          onChange={(e) => {
            const next = limitSeoEdit(value, e.target.value, MAX_CANONICAL);
            if (next !== value) onChange(next);
          }}
        />
      )}
    </FormField>
  );
}

/** The robots directives: one switch each, a group named by its heading. */
function RobotsGroup({
  value,
  disabled,
  onChange,
}: {
  value: readonly SchemaRobotsDto[];
  disabled: boolean;
  onChange: (robots: SchemaRobotsDto[]) => void;
}) {
  const { t } = useTranslation("editor");
  const labelId = useId();
  const hintId = useId();
  const toggle = (directive: SchemaRobotsDto, on: boolean) =>
    onChange(sortedRobots(on ? [...value, directive] : value.filter((d) => d !== directive)));
  return (
    <div
      role="group"
      aria-labelledby={labelId}
      aria-describedby={hintId}
      data-slot="seo-robots"
      className="flex flex-col gap-1.5"
    >
      <span id={labelId} className="text-sm leading-none font-medium">
        {t("seo.robots")}
      </span>
      <p id={hintId} className="text-xs text-muted-foreground">
        {t("seo.robotsHint")}
      </p>
      <div className="mt-0.5 flex flex-col divide-y rounded-lg border">
        {ROBOTS.map((directive) => (
          <div
            key={directive}
            data-slot="seo-robot"
            data-directive={directive}
            className="px-3 py-2.5"
          >
            <SwitchRow
              label={t(`seo.robotsOption.${directive}`)}
              hint={<code className="font-mono">{directive}</code>}
            >
              {(control) => (
                <Switch
                  {...control}
                  className="mt-0.5 pointer-coarse:after:-inset-y-3.5"
                  checked={value.includes(directive)}
                  disabled={disabled}
                  onCheckedChange={(on) => toggle(directive, on)}
                />
              )}
            </SwitchRow>
          </div>
        ))}
      </div>
    </div>
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
  const limitRef = useSeoLimit(max);
  // As the server counts: code points of the trimmed text.
  const n = seoLength(value);
  const over = n > soft;
  const nf = new Intl.NumberFormat(uiLocale());
  // The live region speaks when the count crosses a limit (into or out of "over the soft
  // limit", or onto the hard one), never per keystroke. It is mounted, empty, from the
  // first render, so its first words are a change a screen reader announces.
  const level = n >= max ? "max" : over ? "over" : "within";
  const [lastLevel, setLastLevel] = useState(level);
  const [said, setSaid] = useState("");
  if (level !== lastLevel) {
    setLastLevel(level);
    setSaid(
      level === "max"
        ? t("seo.liveMax", { n: nf.format(max) })
        : level === "over"
          ? t("seo.liveOver", { n: nf.format(soft) })
          : t("seo.liveWithin", { n: nf.format(soft) }),
    );
  }
  const common = {
    id,
    value,
    disabled,
    placeholder,
    "aria-describedby": error ? `${hintId} ${errorId}` : hintId,
    ...(error ? { "aria-invalid": true } : {}),
    onChange: (e: { target: { value: string } }) => {
      const next = limitSeoEdit(value, e.target.value, max);
      if (next !== value) onChange(next);
    },
  };
  let control: ReactNode;
  if (kind === "title") {
    control = <Input {...common} ref={limitRef} data-slot="seo-title" className="text-sm" />;
  } else {
    control = (
      <Textarea
        {...common}
        ref={limitRef}
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
      <span data-slot={`seo-${kind}-live`} role="status" aria-live="polite" className="sr-only">
        {said}
      </span>
      {error ? (
        <p id={errorId} className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
    </div>
  );
}
