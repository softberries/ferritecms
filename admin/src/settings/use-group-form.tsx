// One settings group's form: its keys' draft, what changed, core's limits, and the save.
// Settings are last-write-wins (07 §1 H-S3, P58), so a save re-reads `GET /settings` and,
// when one of the group's keys differs from the copy the form started from, asks
// "Changed by someone else" (Reload / Overwrite, `staleGuard`, P31). Only the group's
// changed keys are sent. A `422` shows at its field; `mod+s` saves; leaving the group
// (another group, another screen) with unsaved changes asks first.

import { useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useCallback, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { type SettingsDto, getSettings, patchSettings } from "@/api/settings";
import { useStaleAsk } from "@/components/kit/use-stale-ask";
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
import { useLeaveGuard } from "@/editor/leave-guard";
import { DESTRUCTIVE } from "@/lib/destructive";
import { useShortcut } from "@/lib/shortcuts";
import { stableEqual, staleGuard } from "@/lib/stale-guard";
import type { DraftOf, FieldSpec, Problem, SettingKey } from "./fields";

const MODAL = '[data-slot="dialog-overlay"], [data-slot="alert-dialog-overlay"]';

// eslint-disable-next-line @typescript-eslint/no-explicit-any -- each key has its own value type
type Specs = Partial<Record<SettingKey, FieldSpec<any>>>;

/** What the group's frame needs (any group's form). */
export interface FrameForm {
  /** The keys whose value would change. */
  changed: readonly string[];
  dirty: boolean;
  saving: boolean;
  /** A refusal not tied to a field. */
  failure: string | null;
  save: () => Promise<void>;
  /** The stale question and the leave question. */
  dialogs: ReactNode;
  /** The `id` of the group's `<form>` (the first refused field takes the focus). */
  formId: string;
  /** Changes whenever the form starts again from a stored copy. */
  generation: number;
}

export interface GroupForm<S extends Specs> extends FrameForm {
  draft: DraftOf<S>;
  set: <K extends keyof S & SettingKey>(key: K, value: DraftOf<S>[K]) => void;
  /** The message to show at each key: the server's, else core's limit once it changed. */
  errors: Partial<Record<keyof S & SettingKey, string>>;
  changed: (keyof S & SettingKey)[];
}

export interface GroupFormOptions {
  /** The group's name as the stale question puts it ("The Reading settings"). */
  resource: string;
  /** Asked with the patch before anything is sent; `false` stops the save. */
  confirm?: (patch: SettingsDto) => Promise<boolean>;
}

function pick<S extends Specs>(specs: S, settings: SettingsDto): SettingsDto {
  const out: Record<string, unknown> = {};
  for (const key of Object.keys(specs)) out[key] = settings[key as SettingKey] ?? null;
  return out;
}

function readAll<S extends Specs>(specs: S, stored: SettingsDto): DraftOf<S> {
  const out: Record<string, unknown> = {};
  for (const [key, spec] of Object.entries(specs)) {
    out[key] = spec?.read(stored[key as SettingKey] ?? null);
  }
  return out as DraftOf<S>;
}

export function useGroupForm<S extends Specs>(
  specs: S,
  settings: SettingsDto,
  options: GroupFormOptions,
): GroupForm<S> {
  const { t } = useTranslation("admin");
  const qc = useQueryClient();
  const stale = useStaleAsk(options.resource);
  // The group's keys as stored when the form started (the stale check compares them).
  const [base, setBase] = useState<SettingsDto>(() => pick(specs, settings));
  const [seen, setSeen] = useState<SettingsDto>(() => pick(specs, settings));
  const [draft, setDraft] = useState<DraftOf<S>>(() => readAll(specs, pick(specs, settings)));
  const [serverErrors, setServerErrors] = useState<Record<string, string>>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [attempted, setAttempted] = useState(false);
  // Bumped each time the form starts from a stored copy (a reload, a save, a refetch).
  const [generation, setGeneration] = useState(0);
  const busy = useRef(false);
  const formId = useId();

  const keys = Object.keys(specs) as (keyof S & SettingKey)[];
  const spec = (key: SettingKey) => specs[key] as FieldSpec<unknown>;
  const startValue = (key: SettingKey) => spec(key).write(spec(key).read(base[key] ?? null));
  const changed = keys.filter((key) => !stableEqual(spec(key).write(draft[key]), startValue(key)));
  const dirty = changed.length > 0;

  const startFrom = useCallback(
    (stored: SettingsDto) => {
      const mine = pick(specs, stored);
      setBase(mine);
      setDraft(readAll(specs, mine));
      setServerErrors({});
      setFailure(null);
      setAttempted(false);
      setGeneration((g) => g + 1);
    },
    [specs],
  );

  // A refetch with nothing edited moves the form to it; with edits pending, the stale
  // check at save time catches the difference.
  const fresh = pick(specs, settings);
  if (!stableEqual(fresh, seen)) {
    setSeen(fresh);
    if (!dirty) startFrom(fresh);
  }

  const problem = (p: Problem): string => {
    switch (p.code) {
      case "tooLong":
        return t("settings.errors.tooLong", { max: p.max });
      case "control":
        return t("settings.errors.control");
      case "locale":
        return t("settings.errors.locale");
      case "integer":
        return t("settings.errors.integer", { min: p.min, max: p.max });
      case "pickPage":
        return t("settings.errors.pickPage");
      case "permalink": {
        const d = p.detail;
        return "segment" in d
          ? t(`settings.errors.permalink.${d.problem}`, { segment: d.segment })
          : t(`settings.errors.permalink.${d.problem}`);
      }
    }
  };

  const clientErrors: Partial<Record<SettingKey, string>> = {};
  for (const key of keys) {
    if (!attempted && !changed.includes(key)) continue;
    const p = spec(key).check?.(draft[key]) ?? null;
    if (p) clientErrors[key] = problem(p);
  }
  const errors = { ...clientErrors, ...serverErrors } as GroupForm<S>["errors"];

  const set: GroupForm<S>["set"] = (key, value) => {
    setDraft((d) => ({ ...d, [key]: value }));
    setServerErrors((e) => {
      if (!(key in e)) return e;
      const next = { ...e };
      delete next[key];
      return next;
    });
  };

  const save = async () => {
    if (busy.current || !dirty) return;
    setAttempted(true);
    const invalid = keys.filter((key) => spec(key).check?.(draft[key]));
    if (invalid.length > 0) {
      toast.error(t("settings.fixFirst"));
      // The first marked field takes the focus once its message renders.
      window.setTimeout(() => {
        document.getElementById(formId)?.querySelector<HTMLElement>("[aria-invalid=true]")?.focus();
      }, 0);
      return;
    }
    const patch: Record<string, unknown> = {};
    for (const key of changed) patch[key] = spec(key).write(draft[key]);
    busy.current = true;
    try {
      // Asking is not saving: the form stays as it is while the question is open.
      if (options.confirm && !(await options.confirm(patch))) return;
      setSaving(true);
      setFailure(null);
      const guard = await staleGuard({
        reread: getSettings,
        base,
        equal: (mine, theirs) => stableEqual(mine, pick(specs, theirs)),
        ask: stale.ask,
      });
      if (!guard.proceed) {
        if (guard.reloaded) {
          qc.setQueryData(qk.settings, guard.reloaded);
          setSeen(pick(specs, guard.reloaded));
          startFrom(guard.reloaded);
          toast.info(t("settings.reloaded"));
        }
        return;
      }
      const saved = await patchSettings(patch);
      qc.setQueryData(qk.settings, saved);
      setSeen(pick(specs, saved));
      startFrom(saved);
      toast.success(t("settings.saved"));
    } catch (err) {
      reportApiError(err);
      const fields = err instanceof ApiError ? err.fieldErrors() : {};
      const known = Object.fromEntries(
        Object.entries(fields).filter(([key]) => (keys as string[]).includes(key)),
      );
      setServerErrors(known);
      const message = err instanceof ApiError ? err.problem.detail || err.problem.title : null;
      if (Object.keys(known).length === 0) setFailure(message ?? t("settings.notSaved"));
      toast.error(t("settings.notSaved"), message ? { description: message } : {});
    } finally {
      busy.current = false;
      setSaving(false);
    }
  };

  const leave = useLeaveGuard(dirty && !saving);
  useShortcut({
    id: "settings.save",
    keys: "mod+s",
    label: t("settings.shortcuts.save"),
    group: t("settings.shortcuts.group"),
    allowInInput: true,
    run: () => {
      if (document.querySelector(MODAL) !== null) return;
      void save();
    },
  });

  const dialogs = (
    <>
      {stale.dialog}
      <AlertDialog open={leave.blocked} onOpenChange={(o) => (o ? undefined : leave.stay())}>
        <AlertDialogContent data-slot="settings-leave-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("settings.leave.title")}</AlertDialogTitle>
            <AlertDialogDescription>{t("settings.leave.body")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("settings.leave.stay")}</AlertDialogCancel>
            <Button variant="destructive" className={DESTRUCTIVE} onClick={leave.leave}>
              {t("settings.leave.leave")}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );

  return { draft, set, errors, changed, dirty, saving, failure, save, dialogs, formId, generation };
}
