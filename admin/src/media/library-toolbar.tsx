// The media library's filter bar (`library.tsx`): search, kind chips, Mine, Trash and
// the grid/list toggle. Each change is a patch of the URL's search.

import { LayoutGridIcon, ListIcon, Trash2Icon, UserRoundIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { ScrollFade } from "@/components/kit/scroll-fade";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Chip, SearchField } from "@/content/filters";
import {
  MEDIA_KINDS,
  type MediaSearch,
  type MediaView,
  hasFilters,
  searchDisables,
} from "./media-search";

export function Toolbar({
  search,
  view,
  onChange,
  onView,
}: {
  search: MediaSearch;
  view: MediaView;
  onChange: (patch: Partial<Record<keyof MediaSearch, unknown>>) => void;
  onView: (view: MediaView) => void;
}) {
  const { t } = useTranslation("media");
  const trash = search.trash === true;
  const disabled = searchDisables(search);
  return (
    <div className="flex flex-col gap-3 lg:flex-row lg:items-center">
      {/* The trash has no search (trashed items leave the search index). */}
      {trash ? null : (
        <div role="search" className="w-full lg:w-auto lg:shrink-0">
          <SearchField
            label={t("searchLabel")}
            value={search.q ?? ""}
            onSearch={(q) => onChange({ q: q || undefined })}
          />
        </div>
      )}
      {/* Phones: the chips get a row of their own, Trash and the view toggle the next. */}
      <div className="flex min-w-0 flex-1 flex-col gap-3 sm:flex-row sm:items-center sm:gap-2">
        <ScrollFade className="-m-1 min-w-0 flex-1 p-1 md:overflow-visible">
          <div className="flex w-max items-center gap-2 md:w-auto md:flex-wrap">
            <div role="group" aria-label={t("kind.label")} className="flex items-center gap-1.5">
              <Chip
                pressed={search.kind === undefined}
                onClick={() => onChange({ kind: undefined })}
              >
                {t("kind.all")}
              </Chip>
              {MEDIA_KINDS.map((kind) => (
                <Chip
                  key={kind}
                  pressed={search.kind === kind}
                  onClick={() => onChange({ kind: search.kind === kind ? undefined : kind })}
                >
                  {t(`kind.${kind}`)}
                </Chip>
              ))}
            </div>
            <span aria-hidden="true" className="mx-0.5 h-5 w-px bg-border" />
            <Chip
              pressed={search.mine === true}
              disabledReason={disabled ? t("mineBySearch") : undefined}
              onClick={() => onChange({ mine: search.mine ? undefined : true })}
            >
              <UserRoundIcon data-icon="inline-start" />
              {t("mine")}
            </Chip>
            {hasFilters(search) ? (
              <Button
                variant="ghost"
                className="text-muted-foreground"
                onClick={() => onChange({ q: undefined, kind: undefined, mine: undefined })}
              >
                {t("clearFilters")}
              </Button>
            ) : null}
          </div>
        </ScrollFade>
        <div className="flex items-center justify-between gap-2">
          <Chip
            pressed={trash}
            onClick={() => onChange({ trash: trash ? undefined : true, id: undefined })}
          >
            <Trash2Icon data-icon="inline-start" />
            {t("trash.toggle")}
          </Chip>
          <ToggleGroup
            type="single"
            variant="outline"
            value={view}
            aria-label={t("view.label")}
            onValueChange={(v) => {
              if (v === "grid" || v === "list") onView(v);
            }}
          >
            <ToggleGroupItem
              value="grid"
              aria-label={t("view.grid")}
              className="pointer-coarse:size-11"
            >
              <LayoutGridIcon />
            </ToggleGroupItem>
            <ToggleGroupItem
              value="list"
              aria-label={t("view.list")}
              className="pointer-coarse:size-11"
            >
              <ListIcon />
            </ToggleGroupItem>
          </ToggleGroup>
        </div>
      </div>
    </div>
  );
}
