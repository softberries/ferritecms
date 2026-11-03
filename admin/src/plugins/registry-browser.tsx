// The Registry tab of the Plugins screen (M6-2, design 14 §10; `/admin/plugins?tab=registry`):
// the configured registry's cached plugins with a search box; each with its publisher
// (verified: the index is signed), its latest version and description, and Install,
// Update or Installed. Install runs the same report step as an upload, fed by the
// registry. Empty states say when no registry is configured or it was never checked.
// "Check for updates" (the screen's header) fetches the index; nothing here does.
// Every string from the index is text.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { BadgeCheckIcon, LibraryIcon, SearchIcon } from "lucide-react";
import { useDeferredValue, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { qk } from "@/api/keys";
import { type RegistryPluginDto, registryQuery } from "@/api/plugins";
import { Alert } from "@/auth/auth-layout";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { InstallPluginDialog, type RegistrySource } from "./install-flow";

export function RegistryBrowser() {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  const [q, setQ] = useState("");
  const query = useDeferredValue(q.trim());
  const registry = useQuery(registryQuery(query));
  const [installing, setInstalling] = useState<RegistrySource | null>(null);

  let body;
  if (registry.isError && !registry.data) {
    body = <ErrorState error={registry.error} retry={() => void registry.refetch()} />;
  } else if (!registry.data) {
    body = (
      <div aria-busy="true" className="flex flex-col gap-2">
        <Skeleton className="h-16 w-full rounded-lg" />
        <Skeleton className="h-16 w-full rounded-lg" />
      </div>
    );
  } else if (!registry.data.registry) {
    body = (
      <EmptyState
        icon={LibraryIcon}
        title={t("plugins.registry.noRegistryTitle")}
        body={t("plugins.registry.noRegistryBody")}
      />
    );
  } else if (registry.data.registry.serial == null) {
    body = (
      <EmptyState
        icon={LibraryIcon}
        title={t("plugins.registry.notFetchedTitle")}
        body={
          registry.data.registry.last_error
            ? t("plugins.registry.failed", { error: registry.data.registry.last_error })
            : t("plugins.registry.notFetchedBody")
        }
      />
    );
  } else if (registry.data.plugins.length === 0) {
    body = (
      <EmptyState
        icon={SearchIcon}
        title={t("plugins.registry.emptyTitle")}
        body={t("plugins.registry.emptyBody")}
      />
    );
  } else {
    body = (
      <ul
        aria-label={t("plugins.registry.list")}
        data-slot="registry-list"
        className="flex flex-col gap-2"
      >
        {registry.data.plugins.map((p) => (
          <RegistryRow
            key={p.name}
            plugin={p}
            onInstall={(version) => setInstalling({ name: p.name, version })}
          />
        ))}
      </ul>
    );
  }

  const meta = registry.data?.registry;
  return (
    <div data-slot="registry-browser" className="flex flex-col gap-4">
      {meta?.last_error && meta.serial != null ? (
        <Alert tone="warning">{t("plugins.registry.failed", { error: meta.last_error })}</Alert>
      ) : null}
      {meta ? (
        <div className="relative max-w-md">
          <SearchIcon
            aria-hidden="true"
            className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground"
          />
          <Input
            type="search"
            value={q}
            onChange={(e) => setQ(e.currentTarget.value)}
            aria-label={t("plugins.registry.searchLabel")}
            placeholder={t("plugins.registry.searchPlaceholder")}
            className="pl-9 pointer-coarse:min-h-11"
          />
        </div>
      ) : null}
      {body}
      <InstallPluginDialog
        open={installing !== null}
        onOpenChange={(o) => {
          if (!o) setInstalling(null);
        }}
        registry={installing}
        onInstalled={(p) => {
          toast.success(t("plugins.install.installed", { name: p.name }));
          void queryClient.invalidateQueries({ queryKey: qk.plugins });
          void queryClient.invalidateQueries({ queryKey: qk.pluginRegistryAll });
        }}
      />
    </div>
  );
}

function RegistryRow({
  plugin: p,
  onInstall,
}: {
  plugin: RegistryPluginDto;
  onInstall: (version: string) => void;
}) {
  const { t } = useTranslation("admin");
  const installed = p.installed;
  const latest = p.latest;
  let action;
  if (!latest) {
    action = null;
  } else if (!installed) {
    action = (
      <Button
        size="sm"
        className="pointer-coarse:min-h-11"
        aria-label={`${t("plugins.registry.install")} ${p.name}`}
        onClick={() => onInstall(latest)}
      >
        {t("plugins.registry.install")}
      </Button>
    );
  } else if (installed.update_available) {
    action = (
      <Button
        size="sm"
        variant="outline"
        className="pointer-coarse:min-h-11"
        aria-label={t("plugins.trust.updateLabel", { name: p.name, version: latest })}
        onClick={() => onInstall(latest)}
      >
        {t("plugins.registry.update")}
      </Button>
    );
  } else {
    action = (
      <span data-slot="registry-installed" className="text-sm text-muted-foreground">
        {t("plugins.registry.installed")}
      </span>
    );
  }
  return (
    <li
      data-slot="registry-plugin"
      data-name={p.name}
      className="flex min-w-0 flex-wrap items-start gap-3 rounded-lg border bg-card px-3 py-3"
    >
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex min-w-0 flex-wrap items-baseline gap-x-2">
          <span className="font-medium break-all text-foreground">{p.name}</span>
          {latest ? (
            <span className="text-xs text-muted-foreground tabular-nums">
              {t("plugins.registry.latest", { version: latest })}
            </span>
          ) : null}
        </span>
        <span className="inline-flex min-w-0 items-center gap-1 text-xs text-muted-foreground">
          <BadgeCheckIcon aria-hidden="true" className="size-3.5 shrink-0 text-success" />
          <span className="truncate">
            {t("plugins.trust.list.verified", { publisher: p.publisher.name })}
          </span>
        </span>
        {p.description ? (
          <span className="text-sm break-words text-muted-foreground">{p.description}</span>
        ) : null}
      </div>
      <div className="shrink-0">{action}</div>
    </li>
  );
}
