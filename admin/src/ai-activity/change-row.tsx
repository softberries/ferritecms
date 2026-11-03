// One change of a set (design 17 §11): the op verb, the object's label linked to its
// screen, the time and its state (reverted by whom, forced, too large); it expands to
// the diff, loaded from `…/changes/{seq}`, and offers "Revert this change".

import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ChevronDownIcon, RotateCcwIcon } from "lucide-react";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { type ChangeDto, changeQuery } from "@/api/change-sets";
import { ErrorState } from "@/components/kit/error-state";
import { RelativeTime } from "@/components/kit/relative-time";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { ContentChangeDiff } from "./content-change-diff";
import { JsonDiff } from "./json-diff";
import { isContentKind } from "./model";

export function ChangeRow({
  setId,
  change,
  names,
  onRevert,
}: {
  setId: string;
  change: ChangeDto;
  names: (id: string) => string | undefined;
  onRevert: (seq: number) => void;
}) {
  const { t } = useTranslation("ai");
  const [open, setOpen] = useState(false);
  const panel = useId();
  const verb = t(`activity.ops.${change.op}` as "activity.ops.content.update", {
    defaultValue: change.op,
  });
  const label = change.object.label || change.object.id;
  const reverted = change.reverted_at != null;
  const who = change.reverted_by ? names(change.reverted_by) : undefined;
  return (
    <li data-slot="change-row" data-seq={change.seq} className="flex min-w-0 flex-col">
      <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1.5 py-3">
        <Button
          variant="ghost"
          size="icon"
          aria-expanded={open}
          aria-controls={panel}
          aria-label={t("activity.showLabel", { label })}
          onClick={() => setOpen((o) => !o)}
          className="shrink-0"
        >
          <ChevronDownIcon
            aria-hidden="true"
            className={open ? "rotate-180 transition-transform" : "transition-transform"}
          />
        </Button>
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="flex min-w-0 flex-wrap items-baseline gap-x-1.5 text-sm">
            <span className="text-muted-foreground">{verb}</span>
            {change.object.admin_path ? (
              <Link
                to={change.object.admin_path}
                className="truncate font-medium text-foreground underline-offset-4 hover:underline"
              >
                {label}
              </Link>
            ) : (
              <span className="truncate font-medium text-foreground">{label}</span>
            )}
          </span>
          <span className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
            <RelativeTime at={change.at} />
            {change.created ? <Badge variant="outline">{t("activity.created")}</Badge> : null}
            {change.deleted ? <Badge variant="outline">{t("activity.deleted")}</Badge> : null}
            {reverted && change.reverted_at ? (
              <span data-slot="change-reverted" className="text-foreground">
                {who ? t("activity.reverted", { who }) : t("activity.revertedAnon")}{" "}
                <RelativeTime at={change.reverted_at} />
              </span>
            ) : null}
            {change.forced ? <Badge variant="outline">{t("activity.forced")}</Badge> : null}
            {!change.revertable ? <span>{t("activity.tooLarge")}</span> : null}
          </span>
        </div>
        {!reverted && change.revertable ? (
          <Button
            variant="outline"
            aria-label={t("activity.revertOneLabel", { seq: change.seq, label })}
            onClick={() => onRevert(change.seq)}
            className="shrink-0"
          >
            <RotateCcwIcon data-icon="inline-start" />
            {t("activity.revertOne")}
          </Button>
        ) : null}
      </div>
      {open ? (
        <div id={panel} className="mb-3 ms-12 min-w-0 rounded-lg border bg-muted/30 p-4">
          <ChangeDiff setId={setId} change={change} />
        </div>
      ) : null}
    </li>
  );
}

function ChangeDiff({ setId, change }: { setId: string; change: ChangeDto }) {
  const { t } = useTranslation("ai");
  const detail = useQuery(changeQuery(setId, change.seq));
  if (detail.isError && !detail.data) {
    return <ErrorState error={detail.error} retry={() => void detail.refetch()} />;
  }
  if (!detail.data) {
    return (
      <div aria-busy="true" aria-label={t("activity.loading")} className="flex flex-col gap-2">
        <Skeleton className="h-4 w-1/3" />
        <Skeleton className="h-16 w-full" />
      </div>
    );
  }
  const d = detail.data;
  if (d.images === "hidden") {
    return <p className="text-sm text-muted-foreground">{t("activity.hidden")}</p>;
  }
  return isContentKind(change.object.kind) ? (
    <ContentChangeDiff before={d.before} after={d.after} />
  ) : (
    <JsonDiff before={d.before} after={d.after} />
  );
}
