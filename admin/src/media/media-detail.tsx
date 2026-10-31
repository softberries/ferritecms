// The media detail sheet (`?id=`): a right sheet with a large preview (the image, or the
// type icon with a Download link; an SVG is noted "sanitised on upload" and is only ever
// an `<img src>`), editable Title and Alt text (saved on blur and `mod+s` with
// `If-Match`; a `412` reloads the item and shows the server's value with a toast), the
// facts (type, size, dimensions, original name, who uploaded it and when, the URL with
// copy), the variants (polled every 3 s for up to 60 s while a raster image has none)
// and the item's actions by capability (Global Constraints: `MediaDto` has no
// `allowed`). Untrusted text (title, alt, original name, URL) is rendered as text.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  CopyIcon,
  DownloadIcon,
  RotateCcwIcon,
  ShieldCheckIcon,
  Trash2Icon,
  XCircleIcon,
} from "lucide-react";
import { useEffect, useId, useRef, useState, type ReactNode, type RefObject } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { INLINE_ERRORS, reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { type MediaDto, displayUrl, getMedia, patchMedia } from "@/api/media";
import { ApiError } from "@/api/problem";
import type { MeDto } from "@/auth/session";
import { ErrorState } from "@/components/kit/error-state";
import { MediaThumb } from "@/components/kit/media-thumb";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { Textarea } from "@/components/ui/textarea";
import { useShortcut } from "@/lib/shortcuts";
import { authorName } from "./author";
import { dimensions, displayTitle, formatBytes, isRaster, typeLabel } from "./format";
import type { MediaAction } from "./media-actions";
import { canEditMedia, canRestoreMedia } from "./rights";
import { VariantsList } from "./variants-list";

/** How often the sheet re-reads an image whose variants are not made yet, and how long. */
export const VARIANTS_POLL_MS = 3_000;
export const VARIANTS_POLL_FOR_MS = 60_000;

type Field = "title" | "alt";
const FIELDS: readonly Field[] = ["title", "alt"];

export interface MediaDetailProps {
  id: string;
  me: MeDto;
  onClose: () => void;
  /** Trash, restore or delete (the library runs them, with Undo or a confirm). */
  onAction?: (action: MediaAction, media: MediaDto) => void;
}

export function MediaDetail({ id, me, onClose, onAction }: MediaDetailProps) {
  const { t } = useTranslation("media");
  const [since] = useState(() => Date.now());
  const [expired, setExpired] = useState(false);
  useEffect(() => {
    const timer = window.setTimeout(() => setExpired(true), VARIANTS_POLL_FOR_MS);
    return () => window.clearTimeout(timer);
  }, []);
  const item = useQuery({
    queryKey: qk.media.item(id),
    queryFn: () => getMedia(id),
    meta: INLINE_ERRORS,
    refetchInterval: (query) => {
      const m = query.state.data;
      const waiting = m !== undefined && isRaster(m.mime) && m.variants.length === 0;
      return waiting && Date.now() - since < VARIANTS_POLL_FOR_MS ? VARIANTS_POLL_MS : false;
    },
  });
  const media = item.data;
  // The fields' saveAll: closing the sheet (Esc, the close button, outside) saves a
  // typed Title or Alt text first, as leaving the field would (no blur fires on removal).
  const saveAll = useRef<(() => Promise<void>) | null>(null);

  return (
    <Sheet
      open
      onOpenChange={(open) => {
        if (open) return;
        void saveAll.current?.();
        onClose();
      }}
    >
      <SheetContent
        side="right"
        data-slot="media-detail"
        className="w-full gap-0 overflow-y-auto p-0 [--sheet-title-center:33px] data-[side=right]:w-full data-[side=right]:sm:max-w-lg"
      >
        {media ? (
          <Detail
            media={media}
            me={me}
            polling={!expired}
            onAction={onAction}
            saveAllRef={saveAll}
          />
        ) : (
          <>
            <SheetHeader className="border-b p-5 pr-12">
              <SheetTitle>{t("detail.loadingTitle")}</SheetTitle>
              <SheetDescription className="sr-only">{t("detail.description")}</SheetDescription>
            </SheetHeader>
            <div className="p-5">
              {item.isError ? (
                <ErrorState
                  error={item.error}
                  retry={() => {
                    void item.refetch();
                  }}
                />
              ) : (
                <div aria-busy="true" className="flex flex-col gap-4">
                  <Skeleton className="aspect-[4/3] w-full rounded-lg" />
                  <Skeleton className="h-9 w-full" />
                  <Skeleton className="h-20 w-full" />
                </div>
              )}
            </div>
          </>
        )}
      </SheetContent>
    </Sheet>
  );
}

function Detail({
  media,
  me,
  polling,
  onAction,
  saveAllRef,
}: {
  media: MediaDto;
  me: MeDto;
  polling: boolean;
  onAction: MediaDetailProps["onAction"];
  saveAllRef: RefObject<(() => Promise<void>) | null>;
}) {
  const { t } = useTranslation("media");
  const trashed = media.status === "trash";
  const editable = canEditMedia(me, media) && !trashed;
  const title = displayTitle(media);
  const size = formatBytes(media.bytes);
  const dims = dimensions(media);
  const author = authorName(media);
  const fields = useFields(media, editable);
  useEffect(() => {
    saveAllRef.current = fields.saveAll;
  });

  useShortcut({
    id: "media.save",
    keys: "mod+s",
    label: t("shortcuts.save"),
    group: t("shortcuts.group"),
    allowInInput: true,
    when: () => editable,
    run: () => {
      void fields.saveAll();
    },
  });

  return (
    <div className="flex flex-col">
      <SheetHeader className="gap-1 border-b p-5 pr-12">
        <SheetTitle className="truncate text-lg" title={title}>
          {title}
        </SheetTitle>
        <SheetDescription className="tabular-nums">
          {[typeLabel(media), size, dims].filter(Boolean).join(" · ")}
        </SheetDescription>
      </SheetHeader>

      <div className="flex flex-col gap-6 p-5">
        <Preview media={media} />

        <section aria-labelledby="media-detail-edit" className="flex flex-col gap-4">
          <h3 id="media-detail-edit" className="sr-only">
            {t("detail.edit")}
          </h3>
          <FieldRow
            field="title"
            label={t("detail.title")}
            fields={fields}
            editable={editable}
            render={(props) => <Input {...props} maxLength={200} />}
          />
          <FieldRow
            field="alt"
            label={t("detail.alt")}
            hint={media.mime.startsWith("image/") ? t("detail.altHint") : t("detail.altHintFile")}
            fields={fields}
            editable={editable}
            render={(props) => <Textarea {...props} rows={3} maxLength={1000} />}
          />
        </section>

        <section aria-labelledby="media-detail-facts" className="flex flex-col gap-3">
          <h3 id="media-detail-facts" className="text-sm font-medium text-muted-foreground">
            {t("detail.facts")}
          </h3>
          <dl className="grid grid-cols-[minmax(7rem,auto)_1fr] gap-x-4 gap-y-2 text-sm">
            <Fact term={t("facts.type")}>
              <span className="font-mono text-xs break-all">{media.mime}</span>
            </Fact>
            <Fact term={t("facts.size")}>
              <span className="tabular-nums">{size}</span>
            </Fact>
            <Fact term={t("facts.dimensions")}>
              <span className="tabular-nums">{dims ?? t("facts.unknown")}</span>
            </Fact>
            <Fact term={t("facts.originalName")}>
              <span className="break-all [font-variant-ligatures:no-contextual]">
                {media.original_name}
              </span>
            </Fact>
            <Fact term={t("facts.uploadedBy")}>{author}</Fact>
            <Fact term={t("facts.uploaded")}>
              <RelativeTime at={media.created_at} />
            </Fact>
            <Fact term={t("facts.url")}>
              <CopyUrl url={media.url} />
            </Fact>
          </dl>
        </section>

        <section aria-labelledby="media-detail-variants" className="flex flex-col gap-3">
          <h3 id="media-detail-variants" className="text-sm font-medium text-muted-foreground">
            {t("variants.title")}
          </h3>
          <VariantsList media={media} polling={polling} />
        </section>

        <Actions media={media} me={me} onAction={onAction} />
      </div>
    </div>
  );
}

/** The image, or its type icon with a Download link. */
function Preview({ media }: { media: MediaDto }) {
  const { t } = useTranslation("media");
  const trashed = media.status === "trash";
  const image = media.mime.startsWith("image/") && !trashed;
  return (
    <figure className="m-0 flex flex-col gap-2">
      {image ? (
        <div className="flex max-h-[min(24rem,50svh)] items-center justify-center overflow-hidden rounded-lg border bg-muted">
          {/* An SVG is an `<img>` too: never inlined (Global Constraints). */}
          <img
            src={media.mime === "image/svg+xml" ? media.url : displayUrl(media)}
            alt={media.alt}
            className="max-h-[min(24rem,50svh)] w-auto max-w-full object-contain"
          />
        </div>
      ) : (
        <div className="flex flex-col items-center gap-4 rounded-lg border bg-muted px-6 py-10">
          <MediaThumb media={media} size="lg" caption={false} />
          {trashed ? (
            <p className="text-center text-sm text-muted-foreground">{t("detail.trashed")}</p>
          ) : (
            <Button asChild variant="outline">
              <a href={media.url} download={media.original_name}>
                <DownloadIcon data-icon="inline-start" />
                {t("detail.download")}
              </a>
            </Button>
          )}
        </div>
      )}
      {media.sanitized ? (
        <figcaption className="flex items-center gap-1.5 text-sm text-muted-foreground">
          <ShieldCheckIcon aria-hidden="true" className="size-4 shrink-0" />
          {t("detail.sanitized")}
        </figcaption>
      ) : null}
    </figure>
  );
}

function Fact({ term, children }: { term: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-muted-foreground">{term}</dt>
      <dd className="m-0 min-w-0">{children}</dd>
    </>
  );
}

function CopyUrl({ url }: { url: string }) {
  const { t } = useTranslation("media");
  const absolute = new URL(url, window.location.origin).toString();
  return (
    <span className="flex min-w-0 items-center gap-1">
      <span className="min-w-0 truncate font-mono text-xs" title={absolute}>
        {absolute}
      </span>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label={t("detail.copyUrl")}
        onClick={() => {
          navigator.clipboard.writeText(absolute).then(
            () => toast.success(t("detail.copied")),
            () => toast.error(t("detail.copyFailed")),
          );
        }}
      >
        <CopyIcon />
      </Button>
    </span>
  );
}

function Actions({
  media,
  me,
  onAction,
}: {
  media: MediaDto;
  me: MeDto;
  onAction: MediaDetailProps["onAction"];
}) {
  const { t } = useTranslation("media");
  if (!onAction || !canEditMedia(me, media)) return null;
  const trashed = media.status === "trash";
  return (
    // Kept in view at the sheet's bottom edge, however long the details above are.
    <div
      data-slot="media-detail-actions"
      className="sticky bottom-0 -mx-5 -mb-5 flex flex-wrap gap-2 border-t bg-popover px-5 py-4"
    >
      {trashed ? (
        <>
          {canRestoreMedia(me, media) ? (
            <Button variant="outline" onClick={() => onAction("restore", media)}>
              <RotateCcwIcon data-icon="inline-start" />
              {t("actions.restore")}
            </Button>
          ) : null}
          <Button
            variant="outline"
            className="text-destructive hover:text-destructive"
            onClick={() => onAction("delete", media)}
          >
            <XCircleIcon data-icon="inline-start" />
            {t("actions.delete")}
          </Button>
        </>
      ) : (
        <Button
          variant="outline"
          className="text-destructive hover:text-destructive"
          onClick={() => onAction("trash", media)}
        >
          <Trash2Icon data-icon="inline-start" />
          {t("actions.trash")}
        </Button>
      )}
    </div>
  );
}

// ---- Title and Alt text -------------------------------------------------------------

interface Fields {
  value: (f: Field) => string;
  set: (f: Field, v: string) => void;
  save: (f: Field) => Promise<void>;
  saveAll: () => Promise<void>;
  saving: (f: Field) => boolean;
  error: (f: Field) => string | null;
}

/**
 * The two fields' local text. The server's value is adopted while the field holds what
 * it last saw (a poll or a save elsewhere), never over the user's own edit. A save sends
 * only a changed field, with the version the sheet holds; saves run one at a time, so
 * the second goes with the version the first returned.
 */
function useFields(media: MediaDto, editable: boolean): Fields {
  const { t } = useTranslation("media");
  const qc = useQueryClient();
  const [draft, setDraft] = useState({ title: media.title, alt: media.alt });
  const [seen, setSeen] = useState({ title: media.title, alt: media.alt });
  const [busy, setBusy] = useState<Partial<Record<Field, boolean>>>({});
  const [errors, setErrors] = useState<Partial<Record<Field, string>>>({});
  if (seen.title !== media.title || seen.alt !== media.alt) {
    setSeen({ title: media.title, alt: media.alt });
    setDraft((d) => ({
      title: d.title === seen.title ? media.title : d.title,
      alt: d.alt === seen.alt ? media.alt : d.alt,
    }));
  }
  // The latest item and draft, for saves that run after an await.
  const latest = useRef({ media, draft });
  useEffect(() => {
    latest.current = { media, draft };
  });
  const chain = useRef<Promise<void>>(Promise.resolve());

  const saveNow = async (field: Field) => {
    const current = latest.current.media;
    const text = latest.current.draft[field];
    if (!editable || text === current[field]) return;
    setBusy((b) => ({ ...b, [field]: true }));
    setErrors((e) => ({ ...e, [field]: undefined }));
    try {
      const saved = await patchMedia(current.id, current.version, { [field]: text });
      latest.current = { ...latest.current, media: saved };
      qc.setQueryData(qk.media.item(saved.id), saved);
      void qc.invalidateQueries({ queryKey: qk.media.lists });
    } catch (e) {
      if (e instanceof ApiError && e.status === 412) {
        // Someone saved in between: show theirs (the field adopts it) and say so.
        const fresh = await getMedia(current.id).catch(() => null);
        if (fresh) {
          latest.current = { ...latest.current, media: fresh };
          qc.setQueryData(qk.media.item(fresh.id), fresh);
          setDraft((d) => ({ ...d, [field]: fresh[field] }));
        }
        toast.info(t("detail.conflict.title"), {
          description: t("detail.conflict.body", { field: t(`detail.${field}`) }),
        });
        return;
      }
      reportApiError(e);
      const detail =
        e instanceof ApiError
          ? (e.fieldErrors()[field] ?? (e.problem.detail || e.problem.title))
          : t("detail.saveFailed");
      setErrors((x) => ({ ...x, [field]: detail }));
      toast.error(t("detail.notSaved", { field: t(`detail.${field}`) }), { description: detail });
    } finally {
      setBusy((b) => ({ ...b, [field]: false }));
    }
  };
  const save = (field: Field) => {
    chain.current = chain.current.then(() => saveNow(field));
    return chain.current;
  };

  return {
    value: (f) => draft[f],
    set: (f, v) => {
      setDraft((d) => ({ ...d, [f]: v }));
      latest.current = { ...latest.current, draft: { ...latest.current.draft, [f]: v } };
    },
    save,
    saveAll: async () => {
      for (const f of FIELDS) await save(f);
    },
    saving: (f) => busy[f] === true,
    error: (f) => errors[f] ?? null,
  };
}

interface FieldInputProps {
  id: string;
  value: string;
  readOnly: boolean;
  "aria-describedby"?: string;
  "aria-invalid"?: boolean;
  onChange: (e: { target: { value: string } }) => void;
  onBlur: () => void;
}

function FieldRow({
  field,
  label,
  hint,
  fields,
  editable,
  render,
}: {
  field: Field;
  label: string;
  hint?: string;
  fields: Fields;
  editable: boolean;
  render: (props: FieldInputProps) => ReactNode;
}) {
  const { t } = useTranslation("media");
  const id = useId();
  const error = fields.error(field);
  const described = [hint ? `${id}-hint` : null, error ? `${id}-error` : null]
    .filter(Boolean)
    .join(" ");
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor={id}>{label}</Label>
        <span aria-live="polite" className="text-xs text-muted-foreground">
          {fields.saving(field) ? t("detail.saving") : null}
        </span>
      </div>
      {render({
        id,
        value: fields.value(field),
        readOnly: !editable,
        ...(described ? { "aria-describedby": described } : {}),
        ...(error ? { "aria-invalid": true } : {}),
        onChange: (e) => fields.set(field, e.target.value),
        onBlur: () => {
          void fields.save(field);
        },
      })}
      {hint ? (
        <p id={`${id}-hint`} className="text-sm text-muted-foreground">
          {hint}
        </p>
      ) : null}
      {error ? (
        <p id={`${id}-error`} className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
    </div>
  );
}
