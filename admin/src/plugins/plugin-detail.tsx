// One plugin (`/admin/plugins/$name`; `manage_plugins`): its manifest facts (text; the
// homepage a link only for `http:`/`https:`), its state with Enable or Disable, the live
// nodes that loaded it (a failed node with its scrubbed error), its metrics, its
// settings, its permission report (the grant it was approved with) and Delete with the
// two choices of H-P3. A broken plugin (`report: null`, H-P2) says why it does not load.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { cn } from "cn";
import { ExternalLinkIcon, PowerIcon, PowerOffIcon, Trash2Icon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { qk } from "@/api/keys";
import { type PluginDto, disablePlugin, enablePlugin, pluginQuery } from "@/api/plugins";
import { Alert } from "@/auth/auth-layout";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { safeHomepage } from "@/appearance/theme-model";
import { Panel } from "@/dashboard/panel";
import { DeletePluginDialog } from "./delete-plugin-dialog";
import { PermissionReport } from "./permission-report";
import { PluginMetrics } from "./plugin-metrics";
import { forgetPlugin, nodeStatus, pluginState } from "./plugin-model";
import { PluginStateBadge } from "./plugin-parts";
import { PluginSettings } from "./plugin-settings";

export function PluginDetail({ name }: { name: string }) {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const plugin = useQuery(pluginQuery(name));
  const [deleting, setDeleting] = useState(false);
  const toggle = useMutation({
    mutationFn: (p: PluginDto) => (p.enabled ? disablePlugin(p.name) : enablePlugin(p.name)),
    onSuccess: (p) => {
      queryClient.setQueryData(qk.plugin(p.name), p);
      void queryClient.invalidateQueries({ queryKey: qk.plugins });
      toast.success(
        p.enabled
          ? t("plugins.install.enabled", { name: p.name })
          : t("plugins.disabled", { name: p.name }),
      );
    },
  });

  if (plugin.isError && !plugin.data) {
    return (
      <div className="mx-auto w-full max-w-6xl">
        <ErrorState error={plugin.error} retry={() => void plugin.refetch()} />
      </div>
    );
  }
  if (!plugin.data) {
    return (
      <div aria-busy="true" className="mx-auto flex w-full max-w-6xl flex-col gap-5">
        <Skeleton className="h-10 w-64" />
        <Skeleton className="h-5 w-96 max-w-full" />
        <Skeleton className="h-64 w-full rounded-xl" />
      </div>
    );
  }

  const p = plugin.data;
  const state = pluginState(p);
  const homepage = safeHomepage(p.homepage);
  return (
    <div data-slot="plugin-detail" className="mx-auto flex w-full max-w-6xl flex-col gap-5">
      <PageHeader
        title={<span className="break-words">{p.name}</span>}
        description={
          p.description ? (
            <span data-slot="plugin-description" className="break-words">
              {p.description}
            </span>
          ) : undefined
        }
        actions={
          <>
            <Button
              variant="ghost"
              className="text-destructive hover:text-destructive pointer-coarse:min-h-11"
              onClick={() => setDeleting(true)}
            >
              <Trash2Icon data-icon="inline-start" />
              {t("plugins.delete.action")}
            </Button>
            {state === "broken" ? null : (
              <Button
                variant={p.enabled ? "outline" : "default"}
                disabled={toggle.isPending}
                aria-busy={toggle.isPending || undefined}
                onClick={() => toggle.mutate(p)}
              >
                {p.enabled ? (
                  <PowerOffIcon data-icon="inline-start" />
                ) : (
                  <PowerIcon data-icon="inline-start" />
                )}
                {toggle.isPending
                  ? p.enabled
                    ? t("plugins.disabling")
                    : t("plugins.enabling")
                  : p.enabled
                    ? t("plugins.disable")
                    : t("plugins.enable")}
              </Button>
            )}
          </>
        }
      />
      <ul
        data-slot="plugin-facts"
        className="flex flex-wrap items-center gap-x-4 gap-y-2 text-sm text-muted-foreground"
      >
        <li>
          <PluginStateBadge plugin={p} />
        </li>
        <li className="tabular-nums">{t("plugins.facts.version", { version: p.version })}</li>
        {p.publisher ? (
          <li data-slot="plugin-publisher" className="min-w-0 break-words">
            {t("plugins.facts.publisher", { publisher: p.publisher })}
          </li>
        ) : null}
        {p.license ? (
          <li data-slot="plugin-license" className="min-w-0 break-words">
            {t("plugins.facts.license", { license: p.license })}
          </li>
        ) : null}
        {p.homepage ? (
          <li data-slot="plugin-homepage" className="min-w-0 break-all">
            {homepage ? (
              <a
                href={homepage}
                target="_blank"
                rel="noopener noreferrer nofollow"
                className="inline-flex items-center gap-1 rounded-sm underline underline-offset-4 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:outline-none"
              >
                {p.homepage}
                <ExternalLinkIcon aria-hidden="true" className="size-3 shrink-0" />
              </a>
            ) : (
              p.homepage
            )}
          </li>
        ) : null}
        <li>
          {t("plugins.facts.updated")} <RelativeTime at={p.updated_at} />
        </li>
      </ul>
      {state === "broken" ? <Alert tone="error">{t("plugins.broken")}</Alert> : null}
      {state === "disabled" && p.disabled_reason ? (
        <div data-slot="plugin-disabled-reason">
          <Alert tone="warning">{t("plugins.disabledReason", { reason: p.disabled_reason })}</Alert>
        </div>
      ) : null}

      <div className="grid min-w-0 gap-5 lg:grid-cols-[minmax(0,1fr)_22rem]">
        <div className="flex min-w-0 flex-col gap-5">
          <PluginMetrics name={p.name} />
          <section id="settings" className="scroll-mt-(--sticky-top)">
            <Panel
              title={t("plugins.settings.title")}
              description={t("plugins.settings.description")}
            >
              <PluginSettings name={p.name} />
            </Panel>
          </section>
        </div>
        <div className="flex min-w-0 flex-col gap-5">
          <Panel title={t("plugins.nodes.title")} description={t("plugins.nodes.description")}>
            <Nodes plugin={p} />
          </Panel>
          <Panel
            title={t("plugins.permissions.title")}
            description={t("plugins.permissions.description")}
          >
            {p.report ? (
              <PermissionReport report={p.report} markAdded={false} />
            ) : (
              <p className="text-sm text-muted-foreground">{t("plugins.permissions.broken")}</p>
            )}
          </Panel>
        </div>
      </div>
      <DeletePluginDialog
        name={deleting ? p.name : null}
        onClose={() => setDeleting(false)}
        onDeleted={(gone, erased) => {
          setDeleting(false);
          toast.success(
            erased
              ? t("plugins.delete.doneErased", { name: gone })
              : t("plugins.delete.doneKept", { name: gone }),
          );
          // The list without it first (its rows ask for metrics), then off this page,
          // whose queries would refetch a plugin that is gone.
          queryClient.setQueryData<PluginDto[]>(qk.plugins, (old) =>
            old?.filter((x) => x.name !== gone),
          );
          void navigate({ to: "/plugins", ignoreBlocker: true }).then(() => {
            forgetPlugin(queryClient, gone);
          });
        }}
      />
    </div>
  );
}

function Nodes({ plugin }: { plugin: PluginDto }) {
  const { t } = useTranslation("admin");
  if (plugin.nodes.length === 0) {
    return <p className="text-sm text-muted-foreground">{t("plugins.nodes.none")}</p>;
  }
  return (
    <ul data-slot="plugin-nodes" className="flex flex-col gap-2.5">
      {plugin.nodes.map((n) => (
        <li
          key={n.node}
          data-slot="plugin-node"
          data-status={n.status}
          className="flex flex-col gap-1 text-sm"
        >
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span
              aria-hidden="true"
              className={cn(
                "size-2 shrink-0 rounded-full",
                n.status === "loaded"
                  ? "bg-success"
                  : n.status === "failed"
                    ? "bg-destructive"
                    : "bg-muted-foreground/60",
              )}
            />
            <span className="min-w-0 font-medium break-all text-foreground">{n.name}</span>
            <span
              className={
                n.status === "loaded"
                  ? "text-success"
                  : n.status === "failed"
                    ? "text-destructive"
                    : "text-muted-foreground"
              }
            >
              {nodeStatus(t, n.status)}
            </span>
            {n.version ? (
              <span className="text-xs text-muted-foreground tabular-nums">{n.version}</span>
            ) : null}
          </div>
          {n.error ? (
            <p className="pl-4 font-mono text-xs break-words text-muted-foreground">{n.error}</p>
          ) : null}
        </li>
      ))}
    </ul>
  );
}
