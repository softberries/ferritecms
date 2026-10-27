// One theme on the Themes screen: name, version, description and the manifest's author,
// licence and homepage (the theme's own text, so text nodes only; the homepage is a link
// only when it parses as `http:`/`https:`), Active and Built in badges, and the actions.
// The active card also says what each live node serves (H-T1): loaded, or the fallback
// with the node's scrubbed error.

import { cn } from "cn";
import {
  ExternalLinkIcon,
  LoaderCircleIcon,
  PaletteIcon,
  SlidersHorizontalIcon,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ThemeDto } from "@/api/themes";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { safeHomepage } from "./theme-model";

/** What the screen knows about an activation it is following (polling node state). */
export type NodeWatch = "checking" | "pending" | null;

export interface ThemeCardProps {
  theme: ThemeDto;
  onActivate: () => void;
  onCustomize: () => void;
  watch: NodeWatch;
}

export function ThemeCard({ theme, onActivate, onCustomize, watch }: ThemeCardProps) {
  const { t } = useTranslation("admin");
  const homepage = safeHomepage(theme.homepage);
  const facts = [
    theme.menus.length > 0 ? t("themes.facts.menus", { count: theme.menus.length }) : null,
    theme.images.length > 0 ? t("themes.facts.images", { count: theme.images.length }) : null,
  ].filter((f): f is string => f !== null);
  return (
    <Card
      data-slot="theme-card"
      data-theme-name={theme.name}
      data-active={theme.active || undefined}
      className={cn("gap-0 py-0", theme.active && "ring-2 ring-primary/60")}
    >
      <div className="flex flex-col gap-3 p-4">
        <div className="flex items-start gap-3">
          <span
            aria-hidden="true"
            className="flex size-10 shrink-0 items-center justify-center rounded-lg border bg-muted text-md font-medium text-muted-foreground uppercase"
          >
            {theme.name.slice(0, 1)}
          </span>
          <div className="flex min-w-0 flex-1 flex-col gap-0.5">
            <h3 className="text-md font-medium break-words text-foreground">{theme.name}</h3>
            <p className="text-xs text-muted-foreground tabular-nums">
              {t("themes.version", { version: theme.version })}
            </p>
          </div>
          <div className="flex shrink-0 flex-wrap justify-end gap-1.5">
            {theme.active ? <Badge>{t("themes.badge.active")}</Badge> : null}
            {theme.origin === "embedded" ? (
              <Badge variant="outline">{t("themes.badge.builtIn")}</Badge>
            ) : null}
          </div>
        </div>
        <p
          className={cn(
            "text-sm break-words",
            theme.description ? "text-foreground" : "text-muted-foreground",
          )}
        >
          {theme.description ?? t("themes.noDescription")}
        </p>
        <ul
          data-slot="theme-facts"
          className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground"
        >
          {theme.author ? (
            <li className="min-w-0 break-words">{t("themes.by", { author: theme.author })}</li>
          ) : null}
          {theme.license ? (
            <li className="min-w-0 break-words">
              {t("themes.license", { license: theme.license })}
            </li>
          ) : null}
          {theme.homepage ? (
            <li className="min-w-0 break-all">
              {homepage ? (
                <a
                  href={homepage}
                  target="_blank"
                  rel="noopener noreferrer nofollow"
                  className="inline-flex items-center gap-1 rounded-sm underline underline-offset-4 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:outline-none"
                >
                  {theme.homepage}
                  <ExternalLinkIcon aria-hidden="true" className="size-3 shrink-0" />
                </a>
              ) : (
                theme.homepage
              )}
            </li>
          ) : null}
          {facts.map((f) => (
            <li key={f}>{f}</li>
          ))}
          {theme.hash ? (
            <li
              className="font-mono tabular-nums"
              title={t("themes.hashTitle", { hash: theme.hash })}
            >
              {t("themes.hash", { hash: theme.hash.slice(0, 12) })}
            </li>
          ) : null}
        </ul>
        {theme.active ? <NodeStates theme={theme} watch={watch} /> : null}
      </div>
      <div className="mt-auto flex flex-wrap items-center justify-end gap-2 border-t bg-muted/40 px-4 py-3">
        {theme.has_settings ? (
          <Button
            variant="ghost"
            className="pointer-coarse:min-h-11"
            aria-label={t("themes.customizeOn", { name: theme.name })}
            onClick={onCustomize}
          >
            <SlidersHorizontalIcon data-icon="inline-start" />
            {t("themes.customize")}
          </Button>
        ) : null}
        {theme.active ? null : (
          <Button
            variant="outline"
            className="pointer-coarse:min-h-11"
            aria-label={t("themes.activateOn", { name: theme.name })}
            onClick={onActivate}
          >
            <PaletteIcon data-icon="inline-start" />
            {t("themes.activate")}
          </Button>
        )}
      </div>
    </Card>
  );
}

function NodeStates({ theme, watch }: { theme: ThemeDto; watch: NodeWatch }) {
  const { t } = useTranslation("admin");
  return (
    <section
      data-slot="theme-nodes"
      aria-label={t("themes.nodes.title")}
      className="flex flex-col gap-2 rounded-lg border bg-background/60 p-3"
    >
      <h4 className="text-xs font-medium text-muted-foreground">{t("themes.nodes.title")}</h4>
      {theme.nodes.length === 0 ? (
        <p className="text-sm text-muted-foreground">{t("themes.nodes.none")}</p>
      ) : (
        <ul className="flex flex-col gap-2">
          {theme.nodes.map((n) => {
            // A node serving another theme (not yet swapped) is not "loaded" either.
            const loaded = !n.fallback && n.name === theme.name;
            return (
              <li
                key={n.node}
                data-slot="theme-node"
                data-state={loaded ? "loaded" : "fallback"}
                className="flex flex-col gap-1 text-sm"
              >
                <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                  <span
                    aria-hidden="true"
                    className={cn(
                      "size-2 shrink-0 rounded-full",
                      loaded ? "bg-success" : "bg-warning",
                    )}
                  />
                  <span className="min-w-0 font-medium break-all text-foreground">
                    {n.node_name}
                  </span>
                  <span className={loaded ? "text-success" : "text-warning"}>
                    {loaded ? t("themes.nodes.loaded") : t("themes.nodes.fallback")}
                  </span>
                </div>
                {loaded ? null : (
                  <p className="pl-4 text-xs break-words text-muted-foreground">
                    {n.error ?? t("themes.nodes.fallbackHint")}
                  </p>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {watch ? (
        <p
          role="status"
          data-slot="theme-watch"
          data-watch={watch}
          className="flex items-start gap-2 text-xs text-muted-foreground"
        >
          {watch === "checking" ? (
            <LoaderCircleIcon
              aria-hidden="true"
              className="mt-px size-3.5 shrink-0 animate-spin motion-reduce:animate-none"
            />
          ) : null}
          {watch === "checking"
            ? t("themes.nodes.checking", { name: theme.name })
            : t("themes.nodes.pending", { name: theme.name })}
        </p>
      ) : null}
    </section>
  );
}
