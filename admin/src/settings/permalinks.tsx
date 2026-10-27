// Permalinks: the post structure (a preset or a custom one built from the token chips)
// and the page structure, which must hold `%parent_path%`. Each shows a live preview of a
// sample item's URL (`permalink-preview.ts`). Saving a change confirms first (P58):
// every post or page URL moves, and no redirects are made from the old ones (M3).

import { useQuery } from "@tanstack/react-query";
import { useCallback, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { metaQuery } from "@/api/meta";
import type { SettingsDto } from "@/api/settings";
import { FormField } from "@/components/kit/form-field";
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
import { Input } from "@/components/ui/input";
import { DESTRUCTIVE } from "@/lib/destructive";
import { ChoiceCards } from "./choice-cards";
import { structureSpec } from "./fields";
import { GroupFormFrame } from "./group-frame";
import {
  DEFAULT_PAGE_STRUCTURE,
  PERMALINK_TOKENS,
  type PermalinkSample,
  type PermalinkToken,
  appendToken,
  permalinkPreview,
} from "./permalink-preview";
import { useGroupForm } from "./use-group-form";

const SPECS = {
  "permalink.post": structureSpec("post"),
  "permalink.page": structureSpec("page"),
};

/** The post presets (WordPress's names for the same shapes). */
const POST_PRESETS = {
  name: "/%slug%/",
  month: "/%year%/%month%/%slug%/",
  category: "/%category%/%slug%/",
} as const;
type Preset = keyof typeof POST_PRESETS | "custom";

const presetOf = (structure: string): Preset =>
  (Object.keys(POST_PRESETS) as (keyof typeof POST_PRESETS)[]).find(
    (k) => POST_PRESETS[k] === structure,
  ) ?? "custom";

/** The sample items the previews show (today's date, in UTC like core). */
function samples(
  t: (key: "settings.permalinks.samplePost" | "settings.permalinks.sampleCategory") => string,
) {
  const date = new Date().toISOString();
  const post: PermalinkSample = {
    slug: t("settings.permalinks.samplePost"),
    date,
    category: t("settings.permalinks.sampleCategory"),
    parents: [],
  };
  const page: PermalinkSample = { slug: "team", date, category: null, parents: ["about"] };
  return { post, page };
}

export function PermalinksGroup({ settings }: { settings: SettingsDto }) {
  const { t } = useTranslation("admin");
  const site = useQuery(metaQuery);
  const origin = site.data ? site.data.site_url.replace(/\/+$/, "") : "";
  const [asking, setAsking] = useState(false);
  const answer = useRef<((ok: boolean) => void) | null>(null);
  const confirm = useCallback(
    () =>
      new Promise<boolean>((resolve) => {
        answer.current = resolve;
        setAsking(true);
      }),
    [],
  );
  const settle = (ok: boolean) => {
    setAsking(false);
    answer.current?.(ok);
    answer.current = null;
  };
  const form = useGroupForm(SPECS, settings, {
    resource: t("settings.resource", { group: t("settings.groups.permalinks.title") }),
    confirm,
  });
  const post = form.draft["permalink.post"];
  const page = form.draft["permalink.page"];
  // "Custom" stays chosen while its structure happens to match a preset.
  const [custom, setCustom] = useState(() => presetOf(post) === "custom");
  const preset: Preset = custom ? "custom" : presetOf(post);
  const sample = samples(t);
  const off = form.saving;

  return (
    <>
      <GroupFormFrame
        title={t("settings.groups.permalinks.title")}
        description={t("settings.groups.permalinks.description")}
        form={form}
      >
        <div className="flex flex-col gap-3">
          <ChoiceCards<Preset>
            label={t("settings.permalinks.post")}
            slot="permalink-presets"
            value={preset}
            disabled={off}
            onChange={(p) => {
              setCustom(p === "custom");
              if (p !== "custom") form.set("permalink.post", POST_PRESETS[p]);
            }}
            choices={[
              ...(Object.keys(POST_PRESETS) as (keyof typeof POST_PRESETS)[]).map((p) => ({
                value: p,
                title: t(`settings.permalinks.presets.${p}`),
                aside: permalinkPreview(POST_PRESETS[p], sample.post),
              })),
              {
                value: "custom" as const,
                title: t("settings.permalinks.presets.custom"),
                description: t("settings.permalinks.customHint"),
              },
            ]}
          />
          {preset === "custom" ? (
            <StructureField
              slot="permalink-post-custom"
              label={t("settings.permalinks.customStructure")}
              value={post}
              error={form.errors["permalink.post"]}
              disabled={off}
              onChange={(v) => form.set("permalink.post", v)}
            />
          ) : null}
          <Preview
            slot="permalink-post-preview"
            label={t("settings.permalinks.postPreview")}
            origin={origin}
            path={permalinkPreview(post.trim(), sample.post)}
          />
        </div>
        <div className="flex flex-col gap-3 border-t pt-6">
          <StructureField
            slot="permalink-page"
            label={t("settings.permalinks.page")}
            hint={t("settings.permalinks.pageHint", { default: DEFAULT_PAGE_STRUCTURE })}
            value={page}
            error={form.errors["permalink.page"]}
            disabled={off}
            onChange={(v) => form.set("permalink.page", v)}
          />
          <Preview
            slot="permalink-page-preview"
            label={t("settings.permalinks.pagePreview")}
            origin={origin}
            path={permalinkPreview(page.trim(), sample.page, "page")}
          />
        </div>
        <p className="text-sm text-muted-foreground">{t("settings.permalinks.otherTypes")}</p>
      </GroupFormFrame>
      <AlertDialog open={asking} onOpenChange={(o) => (o ? undefined : settle(false))}>
        <AlertDialogContent data-slot="permalink-confirm">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("settings.permalinks.confirm.title")}</AlertDialogTitle>
            <AlertDialogDescription>{t("settings.permalinks.confirm.body")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("settings.permalinks.confirm.cancel")}</AlertDialogCancel>
            <Button variant="destructive" className={DESTRUCTIVE} onClick={() => settle(true)}>
              {t("settings.permalinks.confirm.ok")}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

/** A structure input with the token chips (a chip adds its token as the last segment). */
function StructureField({
  slot,
  label,
  hint,
  value,
  error,
  disabled,
  onChange,
}: {
  slot: string;
  label: string;
  hint?: string;
  value: string;
  error: string | undefined;
  disabled: boolean;
  onChange: (value: string) => void;
}) {
  const { t } = useTranslation("admin");
  const chipsId = useId();
  const input = useRef<HTMLInputElement>(null);
  return (
    <FormField slot={slot} label={label} hint={hint} error={error}>
      {(control) => (
        <div className="flex flex-col gap-2">
          <Input
            {...control}
            ref={input}
            value={value}
            disabled={disabled}
            spellCheck={false}
            autoComplete="off"
            className="font-mono"
            onChange={(e) => onChange(e.currentTarget.value)}
          />
          <div className="flex flex-wrap items-center gap-1.5">
            <span id={chipsId} className="text-xs text-muted-foreground">
              {t("settings.permalinks.tokens")}
            </span>
            <div role="group" aria-labelledby={chipsId} className="flex flex-wrap gap-1.5">
              {PERMALINK_TOKENS.map((token: PermalinkToken) => (
                <Button
                  key={token}
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={disabled}
                  title={t(`settings.permalinks.tokenHints.${token.slice(1, -1) as TokenName}`)}
                  aria-label={t("settings.permalinks.addToken", { token })}
                  className="h-7 rounded-full px-2.5 font-mono text-xs pointer-coarse:h-11"
                  onClick={() => {
                    onChange(appendToken(value, token));
                    input.current?.focus();
                  }}
                >
                  {token}
                </Button>
              ))}
            </div>
          </div>
        </div>
      )}
    </FormField>
  );
}

type TokenName = "slug" | "year" | "month" | "category" | "parent_path";

/** "Preview: https://site/2026/10/sample-post/", or why there is none. */
function Preview({
  slot,
  label,
  origin,
  path,
}: {
  slot: string;
  label: string;
  origin: string;
  path: string | null;
}) {
  const { t } = useTranslation("admin");
  return (
    <div
      data-slot={slot}
      aria-live="polite"
      className="flex min-w-0 flex-col gap-0.5 rounded-lg bg-muted/60 px-3 py-2"
    >
      <span className="text-xs text-muted-foreground">{label}</span>
      {path ? (
        <code className="min-w-0 font-mono text-sm text-foreground wrap-anywhere">
          {origin}
          {path}
        </code>
      ) : (
        <span className="text-sm text-muted-foreground">{t("settings.permalinks.noPreview")}</span>
      )}
    </div>
  );
}
