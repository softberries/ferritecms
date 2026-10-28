// Retained data (P47): deleted plugins whose stored data, settings and sealed secrets are
// still kept (`GET /plugins/retained`), with when each was deleted and when it is erased,
// and "Erase now" (`POST /plugins/{name}/purge`, after a confirm). Installing a plugin of
// the same name before then picks the data up again (H-P3). Shown only when there is any.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { format } from "date-fns";
import { ArchiveIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { purgePlugin, retainedPluginsQuery } from "@/api/plugins";
import { Alert } from "@/auth/auth-layout";
import { ErrorState } from "@/components/kit/error-state";
import { RelativeTime } from "@/components/kit/relative-time";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { DESTRUCTIVE } from "@/lib/destructive";
import { problemText } from "@/structure/taxonomies/terms-model";

export function RetainedData() {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  const retained = useQuery(retainedPluginsQuery);
  const [erasing, setErasing] = useState<string | null>(null);
  const [shown, setShown] = useState<string | null>(null);
  if (erasing && erasing !== shown) setShown(erasing);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const erase = async (name: string) => {
    if (pending) return;
    setPending(true);
    setFailure(null);
    try {
      await purgePlugin(name);
      setErasing(null);
      toast.success(t("plugins.retained.erased", { name }));
      void queryClient.invalidateQueries({ queryKey: qk.pluginsRetained });
    } catch (error) {
      if (error instanceof ApiError && error.status === 404) {
        // Erased meanwhile (another admin, or the 30-day job): nothing is left to do.
        setErasing(null);
        toast.info(t("plugins.retained.alreadyErased", { name }));
        void queryClient.invalidateQueries({ queryKey: qk.pluginsRetained });
        return;
      }
      reportApiError(error);
      setFailure(problemText(error));
    } finally {
      setPending(false);
    }
  };

  if (retained.isError && !retained.data) {
    return (
      <section aria-labelledby="retained-title" className="flex flex-col gap-3">
        <RetainedHeading />
        <ErrorState error={retained.error} retry={() => void retained.refetch()} />
      </section>
    );
  }
  const items = retained.data ?? [];
  if (items.length === 0) return null;
  return (
    <section
      data-slot="retained-data"
      aria-labelledby="retained-title"
      className="flex flex-col gap-3"
    >
      <RetainedHeading />
      <div className="rounded-xl border bg-card">
        <table className="w-full text-sm">
          <thead className="border-b bg-muted/40 text-left text-xs text-muted-foreground">
            <tr>
              <th scope="col" className="px-4 py-2.5 font-medium">
                {t("plugins.retained.name")}
              </th>
              <th scope="col" className="hidden px-4 py-2.5 font-medium sm:table-cell">
                {t("plugins.retained.deleted")}
              </th>
              <th scope="col" className="hidden px-4 py-2.5 font-medium sm:table-cell">
                {t("plugins.retained.erasedAfter")}
              </th>
              <th scope="col" className="px-4 py-2.5 text-right font-medium">
                <span className="sr-only">{t("plugins.retained.actions")}</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {items.map((r) => (
              <tr
                key={r.name}
                data-slot="retained-row"
                data-name={r.name}
                className="border-b last:border-b-0"
              >
                <th
                  scope="row"
                  className="w-full max-w-0 px-4 py-2.5 text-left font-normal sm:w-auto"
                >
                  <span className="block font-mono text-sm break-all text-foreground">
                    {r.name}
                  </span>
                  {/* Below `sm` the dates' columns are hidden: the erase date goes here. */}
                  <span className="block text-xs text-muted-foreground tabular-nums sm:hidden">
                    {t("plugins.retained.erasedOn", {
                      date: format(new Date(r.purge_after), "PP"),
                    })}
                  </span>
                </th>
                <td className="hidden px-4 py-2.5 whitespace-nowrap text-muted-foreground sm:table-cell">
                  <RelativeTime at={r.deleted_at} />
                </td>
                <td className="hidden px-4 py-2.5 whitespace-nowrap text-foreground tabular-nums sm:table-cell">
                  <time dateTime={r.purge_after} title={format(new Date(r.purge_after), "PPpp")}>
                    {format(new Date(r.purge_after), "PP")}
                  </time>
                </td>
                <td className="px-4 py-1.5 text-right">
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-destructive hover:text-destructive pointer-coarse:min-h-11"
                    aria-label={t("plugins.retained.eraseOn", { name: r.name })}
                    onClick={() => {
                      setFailure(null);
                      setErasing(r.name);
                    }}
                  >
                    {t("plugins.retained.erase")}
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <AlertDialog
        open={erasing !== null}
        onOpenChange={(open) => !open && !pending && setErasing(null)}
      >
        <AlertDialogContent data-slot="erase-retained">
          <AlertDialogHeader>
            <AlertDialogTitle className="break-words">
              {t("plugins.retained.confirmTitle", { name: shown ?? "" })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("plugins.retained.confirmBody", { name: shown ?? "" })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          {failure ? <Alert tone="error">{failure}</Alert> : null}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={pending}>{t("plugins.retained.cancel")}</AlertDialogCancel>
            <Button
              variant="destructive"
              className={DESTRUCTIVE}
              disabled={pending}
              aria-busy={pending || undefined}
              onClick={() => {
                if (erasing) void erase(erasing);
              }}
            >
              {pending ? t("plugins.retained.erasing") : t("plugins.retained.confirm")}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}

function RetainedHeading() {
  const { t } = useTranslation("admin");
  return (
    <div className="flex flex-col gap-1">
      <h2 id="retained-title" className="flex items-center gap-2 text-lg text-foreground">
        <ArchiveIcon aria-hidden="true" className="size-4 text-muted-foreground" />
        {t("plugins.retained.title")}
      </h2>
      <p className="max-w-prose text-sm text-muted-foreground">
        {t("plugins.retained.description")}
      </p>
    </div>
  );
}
