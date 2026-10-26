// The editor's side panel (07 §3, 11 §1; after Linear's property sidebar): the item's
// status and publishing, its address, excerpt, terms, parent and order, and the danger
// zone, each edited in place. From 1024 px it is a column beside the text (open by
// default, remembered per browser); below, a sheet from the right. `mod+.` toggles it,
// `Esc` closes it. Everything here is part of the one draft (`useEditorDocument`): the
// fields only call `setMeta`, saving is the editor's. Read-only screens show the values
// without a way to change them.

import { useQuery } from "@tanstack/react-query";
import { XIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { metaQuery } from "@/api/meta";
import { typesQuery } from "@/api/types";
import { useMe } from "@/auth/session";
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
import { DangerZone } from "./danger-zone";
import { ExcerptField } from "./excerpt-field";
import { useTypeTaxonomies } from "./panel-state";
import { ParentField } from "./parent-field";
import { SlugField } from "./slug-field";
import { StatusSection } from "./status-section";
import { TermsSection } from "./terms-section";

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
}

export function SidePanel({
  doc,
  type,
  typeLabel,
  open,
  onOpenChange,
  wide,
  onPublish,
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
          <PanelBody doc={doc} type={type} onPublish={onPublish} />
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
          <PanelBody doc={doc} type={type} onPublish={onPublish} />
        </div>
      </SheetContent>
    </Sheet>
  );
}

function PanelBody({
  doc,
  type,
  onPublish,
}: {
  doc: EditorDocument;
  type: string;
  onPublish: (mode: "now" | "schedule") => void;
}) {
  const { t } = useTranslation("editor");
  const me = useMe();
  const site = useQuery(metaQuery);
  const types = useQuery(typesQuery);
  const taxonomies = useTypeTaxonomies(type);
  const hierarchical = types.data?.find((ty) => ty.name === type)?.hierarchical === true;
  const disabled = doc.readOnly !== null;
  const canCreate = me?.capabilities.includes("manage_terms") === true;
  const siteUrl = site.data?.site_url ?? null;

  return (
    <div data-slot="panel-body" className="flex flex-col gap-5 px-5 pt-1 pb-8">
      <StatusSection doc={doc} onPublish={onPublish} />
      <Separator />
      <section aria-labelledby="panel-url" className="flex flex-col gap-4">
        <h3 id="panel-url" className="sr-only">
          {t("panel.sections.url")}
        </h3>
        <SlugField doc={doc} siteUrl={siteUrl} disabled={disabled} />
        <ExcerptField doc={doc} disabled={disabled} />
      </section>
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
      {doc.item?.allowed.trash && !disabled ? (
        <>
          <Separator />
          <DangerZone doc={doc} type={type} />
        </>
      ) : null}
    </div>
  );
}
