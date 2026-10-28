// Media sizes, read-only (P50): the active theme's `[images]` (`ThemeDto.images`), each
// size's name, width × height and crop. Sizes are not a setting: changing them means
// changing the theme. A size the image processor skips (a height without crop, a zero
// dimension, or one of core's own names) says so (`ferrite-http` `variant_specs`).

import { useQuery } from "@tanstack/react-query";
import { ImageIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { INLINE_ERRORS } from "@/api/client";
import { type ThemeDto, themesQuery } from "@/api/themes";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { GroupFrame } from "./group-frame";

type ThemeImage = ThemeDto["images"][number];

/** Core's own sizes (`default_variants`); a theme size of the same name is skipped. */
const CORE_SIZES = ["thumbnail", "medium", "large"];

/** "640 × 360", "1200 × auto", or "640 × 640" for a square crop. */
function sizeText(image: ThemeImage, auto: string): string {
  const height = image.height ?? (image.crop ? image.width : null);
  return `${image.width} × ${height ?? auto}`;
}

/** Why the processor skips the size, or `null` when it makes it. */
function skipped(image: ThemeImage): "noCrop" | "zero" | "taken" | null {
  if (CORE_SIZES.includes(image.name)) return "taken";
  if (image.width === 0 || image.height === 0) return "zero";
  if (image.height != null && !image.crop) return "noCrop";
  return null;
}

export function MediaSizesGroup({ canThemes }: { canThemes: boolean }) {
  const { t } = useTranslation("admin");
  return (
    <GroupFrame
      title={t("settings.groups.mediaSizes.title")}
      description={t("settings.groups.mediaSizes.description")}
    >
      {canThemes ? (
        <Sizes />
      ) : (
        <p className="rounded-lg border border-dashed px-4 py-3 text-sm text-muted-foreground">
          {t("settings.mediaSizes.needsThemes")}
        </p>
      )}
      <p className="max-w-prose text-sm text-muted-foreground">
        {t("settings.mediaSizes.changeNote", { sizes: CORE_SIZES.join(", ") })}
      </p>
    </GroupFrame>
  );
}

function Sizes() {
  const { t } = useTranslation("admin");
  const themes = useQuery({ ...themesQuery, meta: INLINE_ERRORS });
  if (themes.isError) {
    return <ErrorState error={themes.error} retry={() => void themes.refetch()} />;
  }
  if (!themes.data) {
    return (
      <Card aria-busy="true" className="gap-3 p-4">
        <Skeleton className="h-5 w-48" />
        <Skeleton className="h-9 w-full" />
        <Skeleton className="h-9 w-full" />
      </Card>
    );
  }
  const active = themes.data.find((th) => th.active);
  if (!active) {
    return (
      <EmptyState
        icon={ImageIcon}
        title={t("settings.mediaSizes.noActive")}
        body={t("settings.mediaSizes.noActiveBody")}
      />
    );
  }
  return (
    <Card data-slot="media-sizes" className="gap-0 py-0">
      <p className="border-b px-4 py-3 text-sm text-foreground">
        {t("settings.mediaSizes.setBy", { theme: active.name })}
      </p>
      {active.images.length === 0 ? (
        <p className="px-4 py-6 text-sm text-muted-foreground">
          {t("settings.mediaSizes.none", { theme: active.name })}
        </p>
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="pl-4">{t("settings.mediaSizes.name")}</TableHead>
              <TableHead>{t("settings.mediaSizes.size")}</TableHead>
              <TableHead className="pr-4">{t("settings.mediaSizes.crop")}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {active.images.map((image) => {
              const why = skipped(image);
              return (
                <TableRow key={image.name} data-size={image.name}>
                  <TableCell className="pl-4 font-mono text-sm">
                    <span className="flex flex-wrap items-center gap-2">
                      {image.name}
                      {why ? (
                        <Badge variant="outline" title={t(`settings.mediaSizes.skipped.${why}`)}>
                          {t("settings.mediaSizes.notMade")}
                        </Badge>
                      ) : null}
                    </span>
                    {why ? (
                      <span className="mt-1 block font-sans text-xs text-muted-foreground whitespace-normal">
                        {t(`settings.mediaSizes.skipped.${why}`)}
                      </span>
                    ) : null}
                  </TableCell>
                  <TableCell className="font-mono text-sm tabular-nums">
                    {sizeText(image, t("settings.mediaSizes.auto"))}
                  </TableCell>
                  <TableCell className="pr-4 text-sm">
                    {image.crop
                      ? t("settings.mediaSizes.cropped")
                      : t("settings.mediaSizes.scaled")}
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      )}
    </Card>
  );
}
