// The editor's side panel (07 §3, 11 §1; after Linear's property sidebar): the item's
// status and publishing, its address, excerpt, terms, parent and order, and the danger
// zone, each edited in place. From 1024 px it is a column beside the text (open by
// default, remembered per browser); below, a sheet from the right. `mod+.` toggles it,
// `Esc` closes it. Everything here is part of the one draft (`useEditorDocument`): the
// fields only call `setMeta`, saving is the editor's. Read-only screens show the values
// without a way to change them. Under the status, "Revisions (n)" opens the revisions
// drawer (Task 17), for items the server lets this account edit (`allowed.edit`).
// M2b-2: the featured image, the custom fields (unless they are a card under the body)
// and SEO with its search preview.

import { useQuery } from "@tanstack/react-query";
import { ChevronRightIcon, HistoryIcon, XIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { revisionsQuery } from "@/api/content";
import { metaQuery } from "@/api/meta";
import { settingsQuery } from "@/api/settings";
import { typesQuery } from "@/api/types";
import { useMe } from "@/auth/session";
import { KbdCombo } from "@/components/kit/kbd-combo";
import { useSiteLocales } from "@/lib/locales";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import type { EditorDocument } from "../editor-state";
import { FieldsSection } from "../fields/fields-section";
import { DangerZone } from "./danger-zone";
import { ExcerptField } from "./excerpt-field";
import { FeaturedImage } from "./featured-image";
import { useTypeTaxonomies } from "./panel-state";
import { ParentField } from "./parent-field";
import { SeoSection } from "./seo-section";
import { SlugField } from "./slug-field";
import { StatusSection } from "./status-section";
import { TermsSection } from "./terms-section";
import { QualitySection, type QualitySectionProps } from "./quality-section";
import { TranslationsSection } from "./translations-section";

/** The id of the panel's region (the toggle's `aria-controls`). */
export const PANEL_ID = "editor-panel";

export interface SidePanelProps {
  doc: EditorDocument;
  type: string;
  /** The type's label in the singular sense of "settings" ("Posts" → heading). */
  typeLabel: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Beside the text (≥ 1024 px) rather than a sheet. */
  wide: boolean;
  onPublish: (mode: "now" | "schedule") => void;
  /** Opens the revisions drawer; without it there is no "Revisions" entry. */
  onRevisions?: () => void;
  /** The custom fields go in the panel (else nowhere, or a card under the body). */
  fieldsHere: boolean;
  /** The checks (M6-4); without it there is no "Checks" section. */
  quality?: QualitySectionProps;
}

export function SidePanel({
  doc,
  type,
  typeLabel,
  open,
  onOpenChange,
  wide,
  onPublish,
  onRevisions,
  fieldsHere,
  quality,
}: SidePanelProps) {
  const { t } = useTranslation("editor");
  if (wide) {
    if (!open) return null;
    return (
      <aside
        id={PANEL_ID}
        aria-labelledby="editor-panel-title"
        data-slot="editor-panel"
        className="sticky top-[calc(var(--sticky-top)+3rem)] flex h-[calc(100svh-var(--sticky-top)-5.5rem)] w-80 shrink-0 flex-col border-l bg-background xl:w-[22rem]"
      >
        <div className="flex h-11 shrink-0 items-center justify-between gap-2 pr-2 pl-5">
          <h2 id="editor-panel-title" className="text-sm font-semibold">
            {t("panel.title")}
          </h2>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={t("panel.close")}
            className="text-muted-foreground"
            onClick={() => onOpenChange(false)}
          >
            <XIcon />
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain">
          <PanelBody
            doc={doc}
            type={type}
            onPublish={onPublish}
            fieldsHere={fieldsHere}
            {...(onRevisions ? { onRevisions } : {})}
            {...(quality ? { quality } : {})}
          />
        </div>
      </aside>
    );
  }
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        id={PANEL_ID}
        side="right"
        data-slot="editor-panel"
        className="gap-0 p-0 data-[side=right]:w-full data-[side=right]:sm:max-w-md"
      >
        <SheetHeader className="h-14 shrink-0 justify-center border-b px-5 py-0">
          <SheetTitle className="text-sm font-semibold">{t("panel.title")}</SheetTitle>
          <SheetDescription className="sr-only">
            {t("panel.description", { label: typeLabel })}
          </SheetDescription>
        </SheetHeader>
        <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain pt-4 pb-[env(safe-area-inset-bottom)]">
          <PanelBody
            doc={doc}
            type={type}
            onPublish={onPublish}
            fieldsHere={fieldsHere}
            {...(onRevisions ? { onRevisions } : {})}
            {...(quality ? { quality } : {})}
          />
        </div>
      </SheetContent>
    </Sheet>
  );
}

function PanelBody({
  doc,
  type,
  onPublish,
  onRevisions,
  fieldsHere,
  quality,
}: {
  doc: EditorDocument;
  type: string;
  onPublish: (mode: "now" | "schedule") => void;
  onRevisions?: () => void;
  fieldsHere: boolean;
  quality?: QualitySectionProps;
}) {
  const { t } = useTranslation("editor");
  const me = useMe();
  const site = useQuery(metaQuery);
  const types = useQuery(typesQuery);
  const taxonomies = useTypeTaxonomies(type);
  const locales = useSiteLocales();
  const hierarchical = types.data?.find((ty) => ty.name === type)?.hierarchical === true;
  const disabled = doc.readOnly !== null || doc.restoring;
  const canCreate = me?.capabilities.includes("manage_terms") === true;
  const siteUrl = site.data?.site_url ?? null;
  const settings = useQuery(settingsQuery);
  const siteName =
    settings.data?.["site.title"] || (siteUrl ? new URL(siteUrl).host : t("seo.siteFallback"));

  return (
    <div data-slot="panel-body" className="flex flex-col gap-5 px-5 pt-1 pb-8">
      <StatusSection doc={doc} onPublish={onPublish} />
      {onRevisions && doc.item?.allowed.edit ? (
        <RevisionsEntry type={type} id={doc.item.id} onOpen={onRevisions} />
      ) : null}
      <TranslationsPart doc={doc} type={type} />
      {quality ? (
        <>
          <Separator />
          <QualitySection {...quality} />
        </>
      ) : null}
      <Separator />
      <section aria-labelledby="panel-url" className="flex flex-col gap-4">
        <h3 id="panel-url" className="sr-only">
          {t("panel.sections.url")}
        </h3>
        <SlugField doc={doc} siteUrl={siteUrl} disabled={disabled} />
        <ExcerptField doc={doc} disabled={disabled} />
      </section>
      <Separator />
      <FeaturedImage doc={doc} disabled={disabled} />
      {taxonomies.length > 0 ? (
        <>
          <Separator />
          {taxonomies.map((tax) => (
            <TermsSection
              key={tax.name}
              taxonomy={tax}
              selected={doc.meta.terms}
              onChange={(terms) => doc.setMeta({ terms })}
              canCreate={canCreate}
              disabled={disabled}
              locale={locales.multilingual ? doc.item?.locale : undefined}
            />
          ))}
        </>
      ) : null}
      {hierarchical ? (
        <>
          <Separator />
          <section aria-labelledby="panel-parent" className="flex flex-col gap-2">
            <h3 id="panel-parent" className="sr-only">
              {t("panel.sections.parent")}
            </h3>
            <ParentField doc={doc} type={type} disabled={disabled} />
          </section>
        </>
      ) : null}
      {fieldsHere ? (
        <>
          <Separator />
          <FieldsSection doc={doc} />
        </>
      ) : null}
      <Separator />
      <SeoSection doc={doc} siteUrl={siteUrl} siteName={siteName} disabled={disabled} />
      {doc.item?.allowed.trash && !disabled ? (
        <>
          <Separator />
          <DangerZone doc={doc} type={type} />
        </>
      ) : null}
    </div>
  );
}

/** The Translations section with its separator, on a multilingual site only. */
function TranslationsPart({ doc, type }: { doc: EditorDocument; type: string }) {
  const locales = useSiteLocales();
  if (!locales.multilingual || !doc.item) return null;
  return (
    <>
      <Separator />
      <TranslationsSection doc={doc} type={type} />
    </>
  );
}

/** "Revisions (n)": opens the drawer (`mod+alt+r`). */
function RevisionsEntry({ type, id, onOpen }: { type: string; id: string; onOpen: () => void }) {
  const { t } = useTranslation("editor");
  const list = useQuery(revisionsQuery(type, id));
  const n = list.data?.items.length;
  return (
    <button
      type="button"
      data-slot="panel-revisions"
      aria-keyshortcuts="Meta+Alt+R Control+Alt+R"
      title={t("revisions.entryHint")}
      onClick={onOpen}
      className="group -mx-2 -mt-1 flex min-h-11 items-center gap-3 rounded-lg px-2 py-2 text-left text-sm transition-colors duration-(--dur-instant) hover:bg-muted"
    >
      <span
        aria-hidden="true"
        className="flex size-7 shrink-0 items-center justify-center rounded-md border bg-background text-muted-foreground group-hover:text-foreground"
      >
        <HistoryIcon className="size-3.5" />
      </span>
      <span className="min-w-0 flex-1 font-medium text-foreground tabular-nums">
        {n === undefined ? t("revisions.entryLoading") : t("revisions.entry", { n })}
      </span>
      <KbdCombo keys="mod+alt+r" className="max-sm:hidden" />
      <ChevronRightIcon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
    </button>
  );
}
