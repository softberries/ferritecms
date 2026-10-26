// The detail sheet's Variants section: each resized copy with its name, size and a link.
// A raster image without variants yet says they are being made (the `media.variants`
// job runs some seconds after the upload; the sheet polls the item meanwhile), and once
// the polling has given up, that they are not there yet. Other kinds have none.

import { ExternalLinkIcon, LoaderCircleIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { MediaDto } from "@/api/media";
import { isRaster } from "./format";

export interface VariantsListProps {
  media: MediaDto;
  /** Whether the sheet still polls for the variants. */
  polling: boolean;
}

export function VariantsList({ media, polling }: VariantsListProps) {
  const { t } = useTranslation("media");
  if (!isRaster(media.mime)) {
    return <p className="text-sm text-muted-foreground">{t("variants.none")}</p>;
  }
  if (media.variants.length === 0) {
    return (
      <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
        {polling ? (
          <>
            <LoaderCircleIcon
              aria-hidden="true"
              className="size-4 animate-spin motion-reduce:animate-none"
            />
            {t("variants.making")}
          </>
        ) : (
          t("variants.late")
        )}
      </p>
    );
  }
  const sorted = [...media.variants].sort((a, b) => a.width * a.height - b.width * b.height);
  return (
    <ul aria-label={t("variants.title")} className="flex flex-col divide-y rounded-lg border">
      {sorted.map((v) => (
        <li key={v.name} className="flex min-h-11 items-center gap-3 px-3 py-2 text-sm">
          <span className="min-w-0 flex-1 truncate font-medium" title={v.name}>
            {v.name}
          </span>
          <span className="text-muted-foreground tabular-nums">
            {t("variants.size", { width: v.width, height: v.height })}
          </span>
          <a
            href={v.url}
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex min-h-11 items-center gap-1 rounded-sm font-medium underline decoration-muted-foreground/50 underline-offset-4 hover:decoration-foreground"
          >
            {t("variants.open")}
            <span className="sr-only">
              {t("variants.openWhich", { name: v.name })} {t("newTab")}
            </span>
            <ExternalLinkIcon aria-hidden="true" className="size-3.5" />
          </a>
        </li>
      ))}
    </ul>
  );
}
