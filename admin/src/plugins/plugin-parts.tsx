// Small pieces the plugin list and the plugin page share: the state badge and the
// per-node status dots (H-P5: `nodes[]` lists the live nodes that loaded the plugin).

import { cn } from "cn";
import { useTranslation } from "react-i18next";
import type { PluginDto } from "@/api/plugins";
import { type PluginState, nodeStatus, pluginState } from "./plugin-model";

const STATE_TONE: Record<PluginState, string> = {
  enabled: "text-success",
  disabled: "text-muted-foreground",
  broken: "text-destructive",
};

export function PluginStateBadge({ plugin, className }: { plugin: PluginDto; className?: string }) {
  const { t } = useTranslation("admin");
  const state = pluginState(plugin);
  return (
    <span
      data-slot="plugin-state"
      data-state={state}
      className={cn(
        "inline-flex h-6 shrink-0 items-center gap-1.5 rounded-full border px-2 text-xs font-medium whitespace-nowrap",
        STATE_TONE[state],
        className,
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "size-2 shrink-0 rounded-full border-[1.5px] border-current",
          state === "disabled" ? "bg-transparent" : "bg-current",
        )}
      />
      {t(`plugins.state.${state}`)}
    </span>
  );
}

const NODE_TONE: Record<string, string> = {
  loaded: "bg-success",
  failed: "bg-destructive",
  disabled: "bg-muted-foreground/60",
};

/** One dot per live node that reported the plugin; the states as text for screen readers. */
export function NodeDots({ plugin }: { plugin: PluginDto }) {
  const { t } = useTranslation("admin");
  if (plugin.nodes.length === 0) {
    return <span className="text-sm text-muted-foreground">{t("plugins.nodes.noneShort")}</span>;
  }
  const label = plugin.nodes
    .map((n) => t("plugins.nodes.dot", { node: n.name, status: nodeStatus(t, n.status) }))
    .join(", ");
  return (
    <span data-slot="node-dots" title={label} className="inline-flex flex-wrap items-center gap-1">
      {plugin.nodes.map((n) => (
        <span
          key={n.node}
          aria-hidden="true"
          data-status={n.status}
          className={cn(
            "size-2.5 rounded-full ring-2 ring-card",
            NODE_TONE[n.status] ?? "bg-warning",
          )}
        />
      ))}
      <span className="sr-only">{label}</span>
    </span>
  );
}
