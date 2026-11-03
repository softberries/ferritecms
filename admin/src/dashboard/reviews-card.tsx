// "Waiting for your review" (M6-3, design 15 §11.4): how many in-review reviews are assigned
// to the viewer at their current step (`assigned_to_me`) and the oldest three, each opening
// its editor (or its release, for a staged change). Shown to editors and admins only.

import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowRightIcon, ClipboardCheckIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { reviewsQuery } from "@/api/reviews";
import { EmptyState } from "@/components/kit/empty-state";
import { RelativeTime } from "@/components/kit/relative-time";
import { ListSkeleton, Panel, PanelError } from "./panel";

const SHOWN = 3;

export function ReviewsCard({ className }: { className?: string }) {
  const { t } = useTranslation("reviews");
  const list = useQuery(reviewsQuery("assigned", SHOWN));
  const items = list.data?.items.slice(0, SHOWN) ?? [];
  const count = list.data?.assigned_to_me ?? 0;

  let body;
  if (list.isError) body = <PanelError error={list.error} retry={() => void list.refetch()} />;
  else if (list.isPending) body = <ListSkeleton rows={SHOWN} />;
  else if (items.length === 0) {
    body = (
      <EmptyState
        icon={ClipboardCheckIcon}
        title={t("card.empty")}
        body={t("empty.body")}
        className="flex-1 py-8"
      />
    );
  } else {
    body = (
      <div className="flex flex-col gap-3">
        <p className="text-sm font-medium text-foreground tabular-nums">
          {t("card.count", { count })}
        </p>
        <ul className="-mx-2 flex flex-col">
          {items.map((r) => (
            <li key={r.id}>
              <Link
                {...(r.target_kind === "entry" && r.release_id
                  ? { to: "/releases/$id" as const, params: { id: r.release_id } }
                  : {
                      to: "/content/$type/$id" as const,
                      params: { type: r.item_type ?? "post", id: r.item_id },
                    })}
                className="group flex min-h-11 items-center gap-3 rounded-lg px-2 py-2 transition-colors hover:bg-muted/60"
              >
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="truncate font-medium text-foreground">
                    {r.item_title || t("untitled")}
                  </span>
                  <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <span>{r.steps[r.step]?.label ?? ""}</span>
                    <span aria-hidden="true">·</span>
                    <RelativeTime at={r.submitted_at} />
                  </span>
                </span>
                <ArrowRightIcon
                  aria-hidden="true"
                  className="size-4 shrink-0 text-primary transition-transform group-hover:translate-x-0.5"
                />
              </Link>
            </li>
          ))}
        </ul>
      </div>
    );
  }

  return (
    <Panel
      title={t("card.title")}
      busy={list.isPending}
      footer={
        <Link
          to="/reviews"
          search={{ tab: "assigned" }}
          className="inline-flex min-h-6 items-center gap-1 font-medium text-primary hover:underline"
        >
          {t("card.all")}
          <ArrowRightIcon aria-hidden="true" className="size-4" />
        </Link>
      }
      className={className}
    >
      {body}
    </Panel>
  );
}
