// Reading: what `/` shows (the latest posts, or a page picked by search among published
// pages and named through `GET /content/refs`), posts per page (1..=100) and whether
// search engines are asked to stay away (`robots.txt`). A page that is not public keeps
// the post listing at `/` until it is (core's `site.rs`).

import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { ChevronsUpDownIcon, CircleAlertIcon, TriangleAlertIcon } from "lucide-react";
import { useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { INLINE_ERRORS } from "@/api/client";
import { listContent } from "@/api/content";
import { qk } from "@/api/keys";
import type { SettingsDto } from "@/api/settings";
import { FormField, SwitchRow } from "@/components/kit/form-field";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";
import { contentTargetQuery } from "@/content/find-item";
import { cn } from "cn";
import { ChoiceCards } from "./choice-cards";
import { DEFAULTS, type FrontPage, LIMITS, boolSpec, frontPageSpec, intSpec } from "./fields";
import { GroupFormFrame } from "./group-frame";
import { useGroupForm } from "./use-group-form";

const SPECS = {
  "reading.front_page": frontPageSpec,
  "reading.per_page": intSpec(LIMITS.perPage.min, LIMITS.perPage.max),
  "reading.discourage_indexing": boolSpec(DEFAULTS.discourageIndexing),
};

/** How long the page search waits after the last keystroke. */
const SEARCH_DEBOUNCE_MS = 200;

export function ReadingGroup({ settings }: { settings: SettingsDto }) {
  const { t } = useTranslation("admin");
  const form = useGroupForm(SPECS, settings, {
    resource: t("settings.resource", { group: t("settings.groups.reading.title") }),
  });
  const front = form.draft["reading.front_page"];
  const off = form.saving;
  return (
    <GroupFormFrame
      title={t("settings.groups.reading.title")}
      description={t("settings.groups.reading.description")}
      form={form}
    >
      <div className="flex flex-col gap-3">
        <ChoiceCards<FrontPage["kind"]>
          label={t("settings.reading.frontPage")}
          slot="front-page-choice"
          value={front.kind}
          disabled={off}
          columns={2}
          onChange={(kind) =>
            form.set(
              "reading.front_page",
              kind === "posts"
                ? { kind: "posts" }
                : { kind: "page", id: front.kind === "page" ? front.id : null },
            )
          }
          choices={[
            {
              value: "posts",
              title: t("settings.reading.latestPosts"),
              description: t("settings.reading.latestPostsHint"),
            },
            {
              value: "page",
              title: t("settings.reading.aPage"),
              description: t("settings.reading.aPageHint"),
            },
          ]}
        />
        {front.kind === "page" ? (
          <PagePicker
            value={front.id}
            error={form.errors["reading.front_page"]}
            disabled={off}
            onChange={(id) => form.set("reading.front_page", { kind: "page", id })}
          />
        ) : null}
      </div>
      <FormField
        slot="setting-reading-per-page"
        label={t("settings.reading.perPage")}
        hint={t("settings.reading.perPageHint", { value: DEFAULTS.readingPerPage })}
        error={form.errors["reading.per_page"]}
      >
        {(control) => (
          <Input
            {...control}
            inputMode="numeric"
            autoComplete="off"
            placeholder={String(DEFAULTS.readingPerPage)}
            value={form.draft["reading.per_page"]}
            disabled={off}
            className="max-w-28 font-mono tabular-nums"
            onChange={(e) => form.set("reading.per_page", e.currentTarget.value)}
          />
        )}
      </FormField>
      <SwitchRow
        label={t("settings.reading.discourage")}
        hint={t("settings.reading.discourageHint")}
      >
        {(control) => (
          <Switch
            {...control}
            checked={form.draft["reading.discourage_indexing"]}
            disabled={off}
            onCheckedChange={(on) => form.set("reading.discourage_indexing", on)}
          />
        )}
      </SwitchRow>
    </GroupFormFrame>
  );
}

/** A combobox over published pages; the chosen one named by `GET /content/refs`. */
function PagePicker({
  value,
  error,
  disabled,
  onChange,
}: {
  value: string | null;
  error: string | undefined;
  disabled: boolean;
  onChange: (id: string) => void;
}) {
  const { t } = useTranslation("admin");
  const labelId = useId();
  const errorId = useId();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [q, setQ] = useState("");
  useEffect(() => {
    const timer = window.setTimeout(() => setQ(search.trim()), SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [search]);
  const filters = q
    ? { q, status: "published", limit: 20 }
    : { status: "published", order: "title" as const, limit: 50 };
  const candidates = useQuery({
    queryKey: qk.content.list("page", filters),
    queryFn: ({ signal }) => listContent("page", filters, { signal }),
    enabled: open,
    placeholderData: keepPreviousData,
    staleTime: 30_000,
  });
  // Its own error below (not a toast); a failure is not a deleted page.
  const chosen = useQuery({
    ...contentTargetQuery(value ?? ""),
    enabled: value !== null,
    meta: INLINE_ERRORS,
  });
  const name =
    value === null
      ? t("settings.reading.pickPage")
      : chosen.data
        ? chosen.data.title || t("settings.reading.untitled")
        : chosen.isError
          ? t("settings.reading.pageUnknown")
          : chosen.isPending
            ? t("settings.reading.loading")
            : t("settings.reading.pageGone");
  const notice =
    value !== null && chosen.data === null
      ? t("settings.reading.pageGoneHint")
      : chosen.data && chosen.data.status !== "published"
        ? t("settings.reading.notPublished")
        : null;

  return (
    <div data-slot="front-page-picker" className="flex flex-col gap-1.5 sm:max-w-md">
      <span id={labelId} className="text-sm font-medium text-foreground">
        {t("settings.reading.page")}
      </span>
      <Popover
        open={open}
        onOpenChange={(o) => {
          setOpen(o);
          if (!o) setSearch("");
        }}
      >
        <PopoverTrigger asChild>
          <Button
            type="button"
            variant="outline"
            disabled={disabled}
            aria-labelledby={`${labelId} ${labelId}-value`}
            aria-invalid={error ? true : undefined}
            aria-describedby={error ? errorId : undefined}
            className="justify-between font-normal"
          >
            <span
              id={`${labelId}-value`}
              className={cn(
                "truncate",
                (value === null || !chosen.data) && "text-muted-foreground",
              )}
              title={chosen.data?.title}
            >
              {name}
            </span>
            <ChevronsUpDownIcon data-icon="inline-end" className="text-muted-foreground" />
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-(--radix-popover-trigger-width) min-w-64 p-0">
          <Command shouldFilter={false}>
            <CommandInput
              value={search}
              onValueChange={setSearch}
              placeholder={t("settings.reading.searchPages")}
            />
            <CommandList>
              <CommandEmpty>
                {candidates.isFetching
                  ? t("settings.reading.loading")
                  : t("settings.reading.noPages")}
              </CommandEmpty>
              <CommandGroup>
                {(candidates.data?.items ?? []).map((p) => (
                  <CommandItem
                    key={p.id}
                    value={p.id}
                    onSelect={() => {
                      onChange(p.id);
                      setOpen(false);
                    }}
                  >
                    <span className="truncate" title={p.title}>
                      {p.title || t("settings.reading.untitled")}
                    </span>
                    <span className="ml-auto truncate font-mono text-xs text-muted-foreground">
                      {p.slug}
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
      {chosen.isError ? (
        <p
          role="alert"
          data-slot="front-page-failed"
          className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-destructive"
        >
          <CircleAlertIcon aria-hidden="true" className="size-3.5 shrink-0" />
          {t("settings.reading.nameFailed")}
          <Button
            type="button"
            variant="link"
            size="sm"
            className="h-auto p-0 pointer-coarse:h-11"
            onClick={() => void chosen.refetch()}
          >
            {t("settings.reading.retry")}
          </Button>
        </p>
      ) : null}
      {notice ? (
        <p
          data-slot="front-page-notice"
          className="flex items-start gap-1.5 text-sm text-muted-foreground"
        >
          <TriangleAlertIcon aria-hidden="true" className="mt-0.5 size-3.5 shrink-0 text-warning" />
          {notice}
        </p>
      ) : null}
      {error ? (
        <p id={errorId} data-slot="form-error" className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
    </div>
  );
}
