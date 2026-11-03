// Languages (M6-1, design 13 §6): the site's locales, `site.locales`, the default first.
// Add a language by its tag (`de`, `pt-BR`) with a label, rename one, make another the
// default (asked first: the old default's URLs gain a prefix, the new one's lose it; no
// redirects are made, ruling P6), or remove one. The server refuses to remove a language
// that still has content (`409`, with the counts in its message).

import { useQueryClient } from "@tanstack/react-query";
import { ArrowUpIcon, PlusIcon, Trash2Icon } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { qk } from "@/api/keys";
import type { SettingsDto } from "@/api/settings";
import { FormField } from "@/components/kit/form-field";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { canonicalTag } from "@/lib/locales";
import { GroupFormFrame } from "./group-frame";
import { type LocaleRow, MAX_LABEL, MAX_LOCALES, localesSpec, readLocales } from "./locale-list";
import { useGroupForm } from "./use-group-form";

const SPECS = { "site.locales": localesSpec };

export function LanguagesGroup({ settings }: { settings: SettingsDto }) {
  const { t } = useTranslation("admin");
  const qc = useQueryClient();
  const [ask, setAsk] = useState<{ from: string; to: string } | null>(null);
  const answer = useRef<((yes: boolean) => void) | null>(null);
  const form = useGroupForm(SPECS, settings, {
    resource: t("settings.resource", { group: t("settings.groups.languages.title") }),
    confirm: (patch) => {
      const next = readLocales(patch["site.locales"]);
      const before = readLocales(settings["site.locales"]);
      const from = before[0]?.tag ?? "";
      const to = next[0]?.tag ?? "";
      if (!from || from === to) return Promise.resolve(true);
      return new Promise<boolean>((resolve) => {
        answer.current = resolve;
        setAsk({ from, to });
      });
    },
  });
  // A save changes what `/meta` lists: the locale controls everywhere follow it.
  const generation = form.generation;
  useEffect(() => {
    if (generation > 0) void qc.invalidateQueries({ queryKey: qk.meta });
  }, [generation, qc]);

  const rows = form.draft["site.locales"];
  const setRows = (next: LocaleRow[]) => form.set("site.locales", next);
  const off = form.saving;
  const [tag, setTag] = useState("");
  const [label, setLabel] = useState("");
  const typed = tag.trim() === "" ? null : canonicalTag(tag);
  const addError =
    tag.trim() === ""
      ? null
      : !typed
        ? t("settings.languages.badTag")
        : rows.some((r) => r.tag.toLowerCase() === typed.toLowerCase())
          ? t("settings.languages.duplicate")
          : rows.length >= MAX_LOCALES
            ? t("settings.languages.tooMany", { max: MAX_LOCALES })
            : null;

  const add = () => {
    if (!typed || addError) return;
    setRows([...rows, { tag: typed, label: label.trim() }]);
    setTag("");
    setLabel("");
  };
  const close = (yes: boolean) => {
    answer.current?.(yes);
    answer.current = null;
    setAsk(null);
  };

  return (
    <GroupFormFrame
      title={t("settings.groups.languages.title")}
      description={t("settings.groups.languages.description")}
      form={form}
    >
      <ul data-slot="language-list" className="flex flex-col divide-y rounded-md border">
        {rows.map((r, i) => (
          <li key={r.tag} data-locale={r.tag} className="flex flex-wrap items-center gap-3 p-3">
            <span className="w-16 shrink-0 font-mono text-sm">{r.tag}</span>
            <Input
              aria-label={t("settings.languages.labelFor", { tag: r.tag })}
              value={r.label}
              disabled={off}
              maxLength={MAX_LABEL}
              placeholder={t("settings.languages.labelPlaceholder")}
              className="min-w-0 flex-1"
              onChange={(e) => {
                const value = e.currentTarget.value;
                setRows(rows.map((x, j) => (j === i ? { ...x, label: value } : x)));
              }}
            />
            {i === 0 ? (
              <span className="rounded-sm bg-muted px-2 py-0.5 text-xs font-medium">
                {t("settings.languages.default")}
              </span>
            ) : (
              <div className="flex gap-1">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  disabled={off}
                  onClick={() => setRows([r, ...rows.filter((_, j) => j !== i)])}
                >
                  <ArrowUpIcon data-icon="inline-start" />
                  {t("settings.languages.makeDefault")}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  disabled={off}
                  aria-label={t("settings.languages.remove", { tag: r.tag })}
                  onClick={() => setRows(rows.filter((_, j) => j !== i))}
                >
                  <Trash2Icon />
                </Button>
              </div>
            )}
          </li>
        ))}
      </ul>
      {form.errors["site.locales"] ? (
        <p role="alert" className="text-sm text-destructive">
          {form.errors["site.locales"]}
        </p>
      ) : null}
      <div className="flex flex-col gap-3 sm:flex-row sm:items-end">
        <FormField
          slot="language-add-tag"
          label={t("settings.languages.tag")}
          hint={t("settings.languages.tagHint")}
          error={addError ?? undefined}
        >
          {(control) => (
            <Input
              {...control}
              value={tag}
              disabled={off}
              autoComplete="off"
              placeholder="de"
              className="max-w-36 font-mono"
              onChange={(e) => setTag(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  add();
                }
              }}
            />
          )}
        </FormField>
        <FormField slot="language-add-label" label={t("settings.languages.label")}>
          {(control) => (
            <Input
              {...control}
              value={label}
              disabled={off}
              maxLength={MAX_LABEL}
              placeholder={t("settings.languages.labelPlaceholder")}
              onChange={(e) => setLabel(e.currentTarget.value)}
            />
          )}
        </FormField>
        <Button
          type="button"
          variant="outline"
          disabled={off || !typed || addError !== null}
          onClick={add}
        >
          <PlusIcon data-icon="inline-start" />
          {t("settings.languages.add")}
        </Button>
      </div>
      <AlertDialog open={ask !== null} onOpenChange={(o) => (o ? undefined : close(false))}>
        <AlertDialogContent data-slot="default-locale-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("settings.languages.changeDefault.title")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("settings.languages.changeDefault.body", {
                from: ask?.from ?? "",
                to: ask?.to ?? "",
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel onClick={() => close(false)}>
              {t("settings.languages.changeDefault.cancel")}
            </AlertDialogCancel>
            <AlertDialogAction onClick={() => close(true)}>
              {t("settings.languages.changeDefault.confirm")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </GroupFormFrame>
  );
}
