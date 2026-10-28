// A `media` field (any kind, since the schema does not say which) and the featured image
// (images only) share `MediaChoice`: empty, a button that opens the media picker; set,
// the item's thumbnail and title with Replace and Remove. The value is the item's UUID;
// an item that is gone or trashed shows "File not found" with its short id and keeps the
// value until it is replaced or removed.

import { FileQuestionIcon, ImagePlusIcon, PaperclipIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "cn";
import { type MediaKind, displayUrl } from "@/api/media";
import { MediaThumb } from "@/components/kit/media-thumb";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { displayTitle, typeLabel } from "@/media/format";
import { openMediaPicker } from "@/media/media-picker";
import { uuidOf } from "@/lib/values";
import { useMediaItem } from "@/media/media-ref";

/** Ids from the field around it (`<label for>`, hint and error). */
export interface MediaChoiceIds {
  id?: string;
  describedBy?: string;
  invalid?: boolean;
}

export interface MediaChoiceProps {
  value: string | null;
  onChange: (id: string | null) => void;
  kind?: MediaKind;
  /** The picker's title. */
  pickTitle: string;
  /** The empty state's button. */
  chooseLabel: string;
  disabled: boolean;
  ids?: MediaChoiceIds;
  /** A large preview (the featured image) instead of a row. */
  large?: boolean;
}

export function MediaChoice({
  value,
  onChange,
  kind,
  pickTitle,
  chooseLabel,
  disabled,
  ids,
  large = false,
}: MediaChoiceProps) {
  const { t } = useTranslation("editor");
  const id = uuidOf(value);
  const item = useMediaItem(id);
  const pick = async () => {
    const media = await openMediaPicker({ ...(kind ? { kind } : {}), title: pickTitle });
    if (media) onChange(media.id);
  };
  const described = {
    ...(ids?.describedBy ? { "aria-describedby": ids.describedBy } : {}),
    ...(ids?.invalid ? { "aria-invalid": true } : {}),
  };
  if (value === null) {
    return (
      <Button
        type="button"
        variant="outline"
        id={ids?.id}
        {...described}
        disabled={disabled}
        onClick={() => void pick()}
        className={cn("justify-start", large && "h-24 w-full border-dashed")}
      >
        {kind === "image" ? (
          <ImagePlusIcon data-icon="inline-start" />
        ) : (
          <PaperclipIcon data-icon="inline-start" />
        )}
        {chooseLabel}
      </Button>
    );
  }
  const media = item.data ?? null;
  const missing = id === null || (item.isSuccess && media === null);
  const title = media ? displayTitle(media) : "";
  return (
    <div
      data-slot="media-choice"
      data-state={missing ? "missing" : media ? "set" : "loading"}
      className={cn("flex min-w-0 gap-3", large ? "flex-col" : "items-center")}
    >
      {media ? (
        large && media.mime.startsWith("image/") ? (
          <div className="overflow-hidden rounded-lg border bg-muted">
            <img
              src={displayUrl(media)}
              alt={media.alt}
              className="aspect-video w-full object-cover"
              decoding="async"
            />
          </div>
        ) : (
          <MediaThumb media={media} size="sm" caption={false} />
        )
      ) : missing ? (
        <span
          className={cn(
            "flex shrink-0 items-center justify-center rounded-md border border-dashed bg-muted text-muted-foreground",
            large ? "aspect-video w-full" : "size-10",
          )}
        >
          <FileQuestionIcon aria-hidden="true" className="size-4" />
        </span>
      ) : (
        <Skeleton className={large ? "aspect-video w-full" : "size-10"} />
      )}
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <div className="flex min-w-0 flex-col">
          {missing ? (
            <span className="truncate text-sm text-foreground" title={value}>
              {t("fields.mediaMissing", { id: value.slice(0, 8) })}
            </span>
          ) : media ? (
            <>
              <span className="truncate text-sm font-medium text-foreground" title={title}>
                {title}
              </span>
              <span className="truncate text-xs text-muted-foreground">{typeLabel(media)}</span>
            </>
          ) : (
            <span className="text-sm text-muted-foreground">{t("fields.mediaLoading")}</span>
          )}
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            id={ids?.id}
            {...described}
            disabled={disabled}
            onClick={() => void pick()}
            className="pointer-coarse:h-11"
          >
            {t("fields.replace")}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={disabled}
            onClick={() => onChange(null)}
            className="text-muted-foreground pointer-coarse:h-11"
          >
            {t("fields.remove")}
          </Button>
        </div>
      </div>
    </div>
  );
}
