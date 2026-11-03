// One change set (design 17 §11): the client, who it acted as, the time span and the
// counts, "Revert all", then the changes newest first, each expandable to its diff.

import { useQuery } from "@tanstack/react-query";
import { format } from "date-fns";
import { RotateCcwIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { type ChangeSetDetailDto, changeSetQuery } from "@/api/change-sets";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { ChangeRow } from "./change-row";
import { AgentMark, SetStatusBadge } from "./marks";
import { revertableCount } from "./model";
import { RevertDialog, type RevertTarget } from "./revert-dialog";

export function ChangeSetScreen({ id }: { id: string }) {
  const set = useQuery(changeSetQuery(id));
  if (set.isError && !set.data) {
    return (
      <div className="mx-auto w-full max-w-5xl">
        <ErrorState error={set.error} retry={() => void set.refetch()} />
      </div>
    );
  }
  if (!set.data) {
    return (
      <div aria-busy="true" className="mx-auto flex w-full max-w-5xl flex-col gap-4">
        <Skeleton className="h-10 w-1/2" />
        <Skeleton className="h-40 w-full" />
      </div>
    );
  }
  return <Loaded detail={set.data} />;
}

const when = (at: string) => format(new Date(at), "PPp");

function Loaded({ detail }: { detail: ChangeSetDetailDto }) {
  const { t } = useTranslation("ai");
  const [target, setTarget] = useState<RevertTarget | null>(null);
  const { set, changes } = detail;
  const people = new Map<string, string>();
  if (set.user) people.set(set.user.id, set.user.display_name);
  const names = (uid: string) => people.get(uid);
  const left = revertableCount(changes);
  return (
    <div data-slot="change-set-screen" className="mx-auto flex w-full max-w-5xl flex-col gap-5">
      <PageHeader
        title={
          <span className="flex min-w-0 items-center gap-3">
            <AgentMark agent={set.agent} />
            <span className="truncate">{set.agent.client}</span>
          </span>
        }
        description={
          <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <SetStatusBadge set={set} />
            <span>
              {set.user ? set.user.display_name : t("activity.deletedUser")}
              {" · "}
              {t("activity.span", { start: when(set.started_at), end: when(set.last_at) })}
              {" · "}
              {t("activity.changes", { count: set.changes })}
              {set.reverted > 0 ? ` · ${t("activity.revertedCount", { count: set.reverted })}` : ""}
            </span>
          </span>
        }
        actions={
          left > 0 ? (
            <Button data-slot="revert-all" onClick={() => setTarget({ setId: set.id })}>
              <RotateCcwIcon data-icon="inline-start" />
              {t("activity.revertAll")}
            </Button>
          ) : undefined
        }
      />
      <Card className="px-4 py-0">
        <ul aria-label={t("activity.list")} className="flex flex-col divide-y">
          {changes.map((c) => (
            <ChangeRow
              key={c.seq}
              setId={set.id}
              change={c}
              names={names}
              onRevert={(seq) => setTarget({ setId: set.id, seqs: [seq] })}
            />
          ))}
        </ul>
      </Card>
      <RevertDialog target={target} names={names} onClose={() => setTarget(null)} />
    </div>
  );
}
