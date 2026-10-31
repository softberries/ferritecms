// The Plugins screen (07 §3 Plugins; `manage_plugins`): every installed plugin with its
// name, version, publisher and description (the manifest's text, rendered as text), its
// state (Enabled, Disabled with the reason when failures disabled it, Broken when its
// stored manifest or grant no longer parses, H-P2), its failures over 24 hours (every
// node's buckets) and a dot per live node; row actions Enable or Disable, Settings and
// Delete; Install plugin (the three-step flow); and Retained data below (P47).

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { cn } from "cn";
import {
  MoreHorizontalIcon,
  PlugIcon,
  PlusIcon,
  PowerIcon,
  PowerOffIcon,
  SlidersHorizontalIcon,
  Trash2Icon,
} from "lucide-react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { qk } from "@/api/keys";
import {
  type PluginDto,
  disablePlugin,
  enablePlugin,
  pluginMetricsQuery,
  pluginsQuery,
} from "@/api/plugins";
import { DataTable } from "@/components/kit/data-table";
import { type DataTableColumn, dataTableColumns } from "@/components/kit/data-table-features";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { uiLocale } from "@/lib/locale";
import { DeletePluginDialog } from "./delete-plugin-dialog";
import { InstallPluginDialog } from "./install-flow";
import { failureCount, forgetPlugin, pluginState } from "./plugin-model";
import { NodeDots, PluginStateBadge } from "./plugin-parts";
import { RetainedData } from "./retained-data";

export function PluginsScreen() {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const plugins = useQuery(pluginsQuery);
  const list = useMemo(
    () => [...(plugins.data ?? [])].sort((a, b) => a.name.localeCompare(b.name)),
    [plugins.data],
  );
  const [installing, setInstalling] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);

  const refresh = () => void queryClient.invalidateQueries({ queryKey: qk.plugins });
  const toggle = useMutation({
    mutationFn: (p: PluginDto) => (p.enabled ? disablePlugin(p.name) : enablePlugin(p.name)),
    onSuccess: (p) => {
      toast.success(
        p.enabled
          ? t("plugins.install.enabled", { name: p.name })
          : t("plugins.disabled", { name: p.name }),
      );
      refresh();
    },
  });
  const open = (p: PluginDto) => void navigate({ to: "/plugins/$name", params: { name: p.name } });
  const columns = useColumns();

  return (
    <div data-slot="plugins-screen" className="mx-auto flex w-full max-w-6xl flex-col gap-8">
      <div className="flex flex-col gap-5">
        <PageHeader
          title={t("plugins.title")}
          description={t("plugins.description")}
          actions={
            <Button onClick={() => setInstalling(true)}>
              <PlusIcon data-icon="inline-start" />
              {t("plugins.install.action")}
            </Button>
          }
        />
        {plugins.isError && !plugins.data ? (
          <ErrorState error={plugins.error} retry={() => void plugins.refetch()} />
        ) : (
          <DataTable
            label={t("plugins.list")}
            columns={columns}
            data={list}
            getRowId={(p) => p.name}
            onRowOpen={open}
            loading={plugins.isPending}
            animateRows
            rowActions={(p) => (
              <DropdownMenu modal={false}>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={t("plugins.rowMenu", { name: p.name })}
                  >
                    <MoreHorizontalIcon />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-52">
                  {pluginState(p) === "broken" ? null : (
                    <DropdownMenuItem disabled={toggle.isPending} onSelect={() => toggle.mutate(p)}>
                      {p.enabled ? <PowerOffIcon /> : <PowerIcon />}
                      {p.enabled ? t("plugins.disable") : t("plugins.enable")}
                    </DropdownMenuItem>
                  )}
                  <DropdownMenuItem
                    onSelect={() =>
                      void navigate({
                        to: "/plugins/$name",
                        params: { name: p.name },
                        hash: "settings",
                      })
                    }
                  >
                    <SlidersHorizontalIcon />
                    {t("plugins.settings.action")}
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem variant="destructive" onSelect={() => setDeleting(p.name)}>
                    <Trash2Icon />
                    {t("plugins.delete.action")}
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            )}
            columnClassName={{
              plugin: "w-full max-w-0 md:w-[46%]",
              state: "hidden sm:table-cell",
              failures: "hidden text-right md:table-cell",
              nodes: "hidden lg:table-cell",
            }}
            empty={
              <EmptyState
                icon={PlugIcon}
                title={t("plugins.empty.title")}
                body={t("plugins.empty.body")}
                action={
                  <Button variant="outline" onClick={() => setInstalling(true)}>
                    {t("plugins.install.action")}
                  </Button>
                }
                className="border-0"
              />
            }
          />
        )}
      </div>
      <RetainedData />
      <InstallPluginDialog
        open={installing}
        onOpenChange={setInstalling}
        onInstalled={(p) => {
          toast.success(t("plugins.install.installed", { name: p.name }));
          refresh();
          void queryClient.invalidateQueries({ queryKey: qk.pluginsRetained });
        }}
        onEnabled={(p) => {
          toast.success(t("plugins.install.enabled", { name: p.name }));
          refresh();
        }}
      />
      <DeletePluginDialog
        name={deleting}
        onClose={() => setDeleting(null)}
        onDeleted={(name, erased) => {
          setDeleting(null);
          toast.success(
            erased
              ? t("plugins.delete.doneErased", { name })
              : t("plugins.delete.doneKept", { name }),
          );
          forgetPlugin(queryClient, name);
        }}
      />
    </div>
  );
}

/** Failures over 24 hours (every node's), from the plugin's metrics. */
function FailureCell({ name }: { name: string }) {
  const metrics = useQuery(pluginMetricsQuery(name));
  const n = failureCount(metrics.data);
  const number = new Intl.NumberFormat(uiLocale());
  return (
    <span
      data-slot="plugin-failures"
      className={cn(
        "block text-right tabular-nums",
        n ? "font-medium text-destructive" : "text-muted-foreground",
      )}
    >
      {n === undefined ? "–" : number.format(n)}
    </span>
  );
}

function useColumns(): DataTableColumn<PluginDto>[] {
  const { t } = useTranslation("admin");
  return useMemo(() => {
    const col = dataTableColumns<PluginDto>();
    return [
      col.accessor("name", {
        id: "plugin",
        header: () => t("plugins.columns.plugin"),
        cell: (c) => {
          const p = c.row.original;
          const by = p.publisher ? t("plugins.by", { publisher: p.publisher }) : null;
          return (
            <div className="flex min-w-0 flex-col gap-0.5 py-2">
              <span className="flex min-w-0 flex-wrap items-baseline gap-x-2">
                <span
                  data-slot="plugin-name"
                  className="truncate font-medium text-foreground"
                  title={p.name}
                >
                  {p.name}
                </span>
                <span className="text-xs text-muted-foreground tabular-nums">{p.version}</span>
              </span>
              {p.description ? (
                <span
                  data-slot="plugin-description"
                  className="line-clamp-2 text-sm whitespace-normal break-words text-muted-foreground"
                  title={p.description}
                >
                  {p.description}
                </span>
              ) : null}
              {by ? (
                <span
                  data-slot="plugin-publisher"
                  className="truncate text-xs text-muted-foreground"
                  title={by}
                >
                  {by}
                </span>
              ) : null}
              {/* Below `sm` the State column is hidden: the badge goes here. */}
              <span className="mt-1 sm:hidden">
                <PluginStateBadge plugin={p} />
              </span>
              {p.disabled_reason && !p.enabled ? (
                <span className="text-xs whitespace-normal break-words text-warning sm:hidden">
                  {p.disabled_reason}
                </span>
              ) : null}
            </div>
          );
        },
      }),
      col.accessor("enabled", {
        id: "state",
        header: () => t("plugins.columns.state"),
        cell: (c) => {
          const p = c.row.original;
          return (
            <div className="flex min-w-0 flex-col items-start gap-1">
              <PluginStateBadge plugin={p} />
              {p.disabled_reason && !p.enabled ? (
                <span
                  data-slot="disabled-reason"
                  className="max-w-56 text-xs whitespace-normal break-words text-warning"
                >
                  {p.disabled_reason}
                </span>
              ) : null}
            </div>
          );
        },
      }),
      col.display({
        id: "failures",
        header: () => <span className="block text-right">{t("plugins.columns.failures")}</span>,
        cell: (c) => <FailureCell name={c.row.original.name} />,
      }),
      col.display({
        id: "nodes",
        header: () => t("plugins.columns.nodes"),
        cell: (c) => <NodeDots plugin={c.row.original} />,
      }),
    ];
  }, [t]);
}
