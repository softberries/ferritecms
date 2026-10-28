// The Plugins screens' rules that are not components.

import type { QueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useEffect, useState } from "react";
import { qk } from "@/api/keys";
import type { PluginDto, PluginMetricsDto } from "@/api/plugins";
import type { SchemaGrantReportDto, SchemaPermissionItemDto } from "@/api/schema";
import { uiLocale } from "@/lib/locale";

/** Plugin metrics are read again this often while the page is visible (exact values). */
export const METRICS_REFRESH_MS = 30_000;

/** Enabled, disabled (by an admin or after failures), or broken (H-P2: `report: null`,
 * its stored manifest or grant no longer parses, so it does not load). */
export type PluginState = "enabled" | "disabled" | "broken";

export function pluginState(p: PluginDto): PluginState {
  if (!p.report) return "broken";
  return p.enabled ? "enabled" : "disabled";
}

/** Failures in the metrics window (24 h), every node's. */
export function failureCount(metrics: PluginMetricsDto | undefined): number | undefined {
  return metrics?.failures.reduce((n, b) => n + b.count, 0);
}

/** One report item as shown: whether it is new on this update, and which detail lines are. */
export interface ShownItem {
  item: SchemaPermissionItemDto;
  isNew: boolean;
  newDetail: ReadonlySet<string>;
}

const sameList = (a: readonly string[], b: readonly string[]) =>
  a.length === b.length && a.every((v, i) => v === b[i]);

/**
 * The items in display order (high risk first, otherwise the server's order) with the
 * `added` ones marked when `markAdded`. An added item matches an item of the same key
 * with the same text and detail (all of it new), or one whose detail holds every added
 * line (a widened list: only those lines new). An added item that matches none is shown
 * on its own, new.
 */
export function shownItems(report: SchemaGrantReportDto, markAdded: boolean): ShownItem[] {
  const added = markAdded ? report.added : [];
  const used = new Set<number>();
  const shown: ShownItem[] = report.items.map((item) => {
    const newDetail = new Set<string>();
    let isNew = false;
    added.forEach((a, i) => {
      if (a.key !== item.key) return;
      if (a.text === item.text && sameList(a.detail, item.detail)) {
        isNew = true;
        used.add(i);
      } else if (a.detail.length > 0 && a.detail.every((d) => item.detail.includes(d))) {
        isNew = true;
        for (const d of a.detail) newDetail.add(d);
        used.add(i);
      }
    });
    return { item, isNew, newDetail };
  });
  added.forEach((a, i) => {
    if (!used.has(i)) shown.push({ item: a, isNew: true, newDetail: new Set() });
  });
  // A stable sort: high risk first.
  return shown
    .map((s, i) => ({ s, i }))
    .sort((x, y) => Number(y.s.item.high_risk) - Number(x.s.item.high_risk) || x.i - y.i)
    .map(({ s }) => s);
}

/** `48 MiB`, as the server words its limit items. */
export function formatMemory(bytes: number): string {
  const MIB = 1024 * 1024;
  const n = new Intl.NumberFormat(uiLocale(), { maximumFractionDigits: 1 });
  if (bytes >= MIB) return `${n.format(bytes / MIB)} MiB`;
  if (bytes >= 1024) return `${n.format(bytes / 1024)} KiB`;
  return `${n.format(bytes)} B`;
}

/** Whether the document is visible (`document.visibilityState`), kept current. */
export function usePageVisible(): boolean {
  const [visible, setVisible] = useState(() => document.visibilityState !== "hidden");
  useEffect(() => {
    const update = () => setVisible(document.visibilityState !== "hidden");
    document.addEventListener("visibilitychange", update);
    return () => document.removeEventListener("visibilitychange", update);
  }, []);
  return visible;
}

/** The query's refetch interval: every 30 s while the page is visible, else none. */
export function metricsInterval(visible: boolean): number | false {
  return visible ? METRICS_REFRESH_MS : false;
}

export function nodeStatus(t: TFunction<"admin">, status: string): string {
  return status === "loaded" || status === "failed" || status === "disabled"
    ? t(`plugins.nodes.status.${status}`)
    : status;
}

/**
 * After plugin `name` was deleted: it leaves the cached list at once (so no row asks for
 * its metrics any more, a `404`), its own queries go, and the list and the retained data
 * are read again. The caller is off the plugin's own page.
 */
export function forgetPlugin(queryClient: QueryClient, name: string): void {
  queryClient.setQueryData<PluginDto[]>(qk.plugins, (old) => old?.filter((p) => p.name !== name));
  queryClient.removeQueries({ queryKey: [...qk.plugins, name] });
  void queryClient.invalidateQueries({ queryKey: qk.plugins, exact: true });
  void queryClient.invalidateQueries({ queryKey: qk.pluginsRetained });
}
