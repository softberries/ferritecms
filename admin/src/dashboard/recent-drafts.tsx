// Recent drafts (every role): the actor's five latest drafts of posts and pages
// (`author=<me.id>`, pre-flight F34; `status=draft`, newest change first), each with a
// way back into the editor.

import { useQueries } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowRightIcon, FilePenLineIcon, PenLineIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { listContent } from "@/api/content";
import { qk } from "@/api/keys";
import { EmptyState } from "@/components/kit/empty-state";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import { ListSkeleton, Panel, PanelError } from "./panel";
import { CARD_LIMIT, draftsFilters, newest } from "./queries";

const TYPES = ["post", "page"] as const;

export function RecentDrafts({ me, canWrite }: { me: string; canWrite: boolean }) {
  const { t } = useTranslation("dashboard");
  const filters = draftsFilters(me);
  const lists = useQueries({
    queries: TYPES.map((type) => ({
      queryKey: qk.content.list(type, filters),
      queryFn: ({ signal }: { signal: AbortSignal }) => listContent(type, filters, { signal }),
      staleTime: 30_000,
    })),
  });
  const failed = lists.find((q) => q.isError);
  const pending = lists.some((q) => q.isPending);
  const items =
    pending || failed
      ? []
      : newest(
          lists.map((q) => q.data?.items ?? []),
          CARD_LIMIT,
        );

  let body;
  if (failed) {
    body = (
      <PanelError
        error={failed.error}
        retry={() => {
          for (const q of lists) if (q.isError) void q.refetch();
        }}
      />
    );
  } else if (pending) {
    body = <ListSkeleton rows={CARD_LIMIT} />;
  } else if (items.length === 0) {
    body = (
      <EmptyState
        icon={FilePenLineIcon}
        title={t("drafts.emptyTitle")}
        body={t("drafts.emptyBody")}
        className="flex-1 py-8"
        action={
          canWrite ? (
            <Button variant="outline" asChild>
              <Link to={"/content/post/new" as "/"}>
                <PenLineIcon data-icon="inline-start" />
                {t("activity.write")}
              </Link>
            </Button>
          ) : undefined
        }
      />
    );
  } else {
    body = (
      <ul className="-mx-2 flex flex-col">
        {items.map((item) => (
          <li key={item.id}>
            <Link
              to={`/content/${item.type}/${item.id}` as "/"}
              className="group flex min-h-11 items-center gap-3 rounded-lg px-2 py-2 transition-colors hover:bg-muted/60"
            >
              <span className="flex min-w-0 flex-1 flex-col">
                <span
                  className="truncate font-medium text-foreground"
                  title={item.title || t("drafts.untitled")}
                >
                  {item.title || t("drafts.untitled")}
                </span>
                <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
                  <span>{t(`drafts.type.${item.type === "page" ? "page" : "post"}`)}</span>
                  <span aria-hidden="true">·</span>
                  <span>
                    {t("drafts.edited")} <RelativeTime at={item.updated_at} />
                  </span>
                </span>
              </span>
              <span className="flex shrink-0 items-center gap-1 text-sm font-medium text-primary">
                <span className="max-sm:sr-only">{t("drafts.continue")}</span>
                <ArrowRightIcon
                  aria-hidden="true"
                  className="size-4 transition-transform group-hover:translate-x-0.5"
                />
              </span>
            </Link>
          </li>
        ))}
      </ul>
    );
  }

  return (
    <Panel
      title={t("drafts.title")}
      description={t("drafts.description")}
      busy={pending && !failed}
      footer={
        items.length > 0 ? (
          <Link
            to={"/content/post?status=draft" as "/"}
            className="inline-flex min-h-6 items-center gap-1 font-medium text-primary hover:underline"
          >
            {t("drafts.all")}
            <ArrowRightIcon aria-hidden="true" className="size-4" />
          </Link>
        ) : undefined
      }
      className="lg:col-span-4"
    >
      {body}
    </Panel>
  );
}
