// The Plugins screen (07 §3 Plugins; `manage_plugins`): every installed plugin with its
// name, version, publisher and description (the manifest's text, rendered as text), its
// state (Enabled, Disabled with the reason when failures disabled it, Broken when its
// stored manifest or grant no longer parses, H-P2), its failures over 24 hours (every
// node's buckets) and a dot per live node; row actions Enable or Disable, Settings and
// Delete; Install plugin (the three-step flow); and Retained data below (P47).
// M6-2: the publisher as words (verified, signed, unsigned, key revoked) and an Update
// chip when the registry offers a newer version (the same report step, fed by the
// registry).

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { cn } from "cn";
import {
  ArrowUpCircleIcon,
  KeyRoundIcon,
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
  refreshRegistry,
  registryQuery,
} from "@/api/plugins";
import { reportApiError } from "@/api/client";
import { RelativeTime } from "@/components/kit/relative-time";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
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
import { InstallPluginDialog, type RegistrySource } from "./install-flow";
import { failureCount, forgetPlugin, pluginState } from "./plugin-model";
import { NodeDots, PluginStateBadge } from "./plugin-parts";
import { RegistryBrowser } from "./registry-browser";
import { RetainedData } from "./retained-data";
import { PublisherLine } from "./trust-summary";

export function PluginsScreen({ tab = "installed" }: { tab?: "installed" | "registry" }) {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const plugins = useQuery(pluginsQuery);
  const list = useMemo(
    () => [...(plugins.data ?? [])].sort((a, b) => a.name.localeCompare(b.name)),
    [plugins.data],
  );
  const [installing, setInstalling] = useState(false);
  const [fromRegistry, setFromRegistry] = useState<RegistrySource | null>(null);
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
  const registry = useQuery(registryQuery(""));
  const meta = registry.data?.registry ?? null;
  const check = useMutation({
    mutationFn: refreshRegistry,
    onSuccess: (data) => {
      toast.success(t("plugins.registry.checkedToast"));
      if (data.collisions.length > 0) {
        toast.warning(
          t("plugins.registry.collisionToast", {
            count: data.collisions.length,
            keys: data.collisions.join(", "),
          }),
        );
      }
      void queryClient.invalidateQueries({ queryKey: qk.pluginRegistryAll });
      refresh();
    },
    onError: (error) => {
      reportApiError(error);
      void queryClient.invalidateQueries({ queryKey: qk.pluginRegistryAll });
    },
  });
  const columns = useColumns((p) => {
    if (p.trust.update) setFromRegistry({ name: p.name, version: p.trust.update.version });
  });

  return (
    <div data-slot="plugins-screen" className="mx-auto flex w-full max-w-6xl flex-col gap-8">
      <div className="flex flex-col gap-5">
        <PageHeader
          title={t("plugins.title")}
          description={t("plugins.description")}
          actions={
            <>
              <Button variant="ghost" asChild className="pointer-coarse:min-h-11">
                <Link to="/plugins/publishers">
                  <KeyRoundIcon data-icon="inline-start" />
                  {t("plugins.publishers.link")}
                </Link>
              </Button>
              {meta ? (
                <Button
                  variant="outline"
                  data-slot="registry-check"
                  disabled={check.isPending}
                  aria-busy={check.isPending || undefined}
                  onClick={() => check.mutate()}
                >
                  {check.isPending ? t("plugins.registry.checking") : t("plugins.registry.check")}
                </Button>
              ) : null}
              <Button onClick={() => setInstalling(true)}>
                <PlusIcon data-icon="inline-start" />
                {t("plugins.install.action")}
              </Button>
            </>
          }
        />
        {meta ? (
          <p data-slot="registry-checked" className="-mt-2 text-xs text-muted-foreground">
            {meta.checked_at ? (
              <>
                {t("plugins.registry.checked")} <RelativeTime at={meta.checked_at} />
              </>
            ) : (
              t("plugins.registry.neverChecked")
            )}
          </p>
        ) : null}
        <Tabs
          value={tab}
          onValueChange={(v) =>
            void navigate({ to: "/plugins", search: v === "registry" ? { tab: "registry" } : {} })
          }
          className="min-w-0 gap-4"
        >
          <TabsList aria-label={t("plugins.registry.tabsLabel")} className="w-max">
            <TabsTrigger value="installed" className="px-3 pointer-coarse:min-h-11">
              {t("plugins.registry.installedTab")}
            </TabsTrigger>
            <TabsTrigger value="registry" className="px-3 pointer-coarse:min-h-11">
              {t("plugins.registry.tab")}
            </TabsTrigger>
          </TabsList>
          <TabsContent value="registry">
            <RegistryBrowser />
          </TabsContent>
          <TabsContent value="installed" className="flex flex-col gap-8">
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
                        <DropdownMenuItem
                          disabled={toggle.isPending}
                          onSelect={() => toggle.mutate(p)}
                        >
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
            <RetainedData />
          </TabsContent>
        </Tabs>
      </div>
      <InstallPluginDialog
        open={fromRegistry !== null}
        onOpenChange={(o) => {
          if (!o) setFromRegistry(null);
        }}
        registry={fromRegistry}
        onInstalled={(p) => {
          toast.success(t("plugins.install.installed", { name: p.name }));
          refresh();
          void queryClient.invalidateQueries({ queryKey: qk.pluginRegistryAll });
        }}
      />
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

/** The Update chip: the registry offers a newer version of this plugin. */
export function UpdateChip({ plugin, onUpdate }: { plugin: PluginDto; onUpdate: () => void }) {
  const { t } = useTranslation("admin");
  const update = plugin.trust.update;
  if (!update) return null;
  return (
    <Button
      variant="outline"
      size="sm"
      data-slot="plugin-update"
      className="h-7 w-fit gap-1 rounded-full px-2.5 text-xs pointer-coarse:min-h-11"
      aria-label={t("plugins.trust.updateLabel", { name: plugin.name, version: update.version })}
      onClick={(e) => {
        e.stopPropagation();
        onUpdate();
      }}
    >
      <ArrowUpCircleIcon aria-hidden="true" className="size-3.5" />
      {t("plugins.trust.update", { version: update.version })}
    </Button>
  );
}

function useColumns(onUpdate: (p: PluginDto) => void): DataTableColumn<PluginDto>[] {
  const { t } = useTranslation("admin");
  return useMemo(() => {
    const col = dataTableColumns<PluginDto>();
    return [
      col.accessor("name", {
        id: "plugin",
        header: () => t("plugins.columns.plugin"),
        cell: (c) => {
          const p = c.row.original;
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
              <PublisherLine plugin={p} />
              {p.trust.update ? <UpdateChip plugin={p} onUpdate={() => onUpdate(p)} /> : null}
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
  }, [t, onUpdate]);
}
