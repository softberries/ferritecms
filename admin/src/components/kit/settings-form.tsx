// A form over a declared settings schema (a theme's `[settings]`, a plugin's; 05 §4c,
// 06 §1): `GET …/settings` gives the schema and every value as read (defaults filled
// in), `PATCH …/settings` takes only the keys to change, `null` resetting one to its
// default (for a secret: deleting it). Kinds: `text`, `textarea`, `bool`, `number`,
// `select`, `color` (`#rrggbb`; a swatch picker plus the hex), `media` (the media picker)
// and `secret` (plugins only, write-only through `SecretField`). Each field shows its
// default and offers "Reset to default" when it differs or its stored value does not fit
// the definition (`invalid`, read as the default until written again).
//
// Settings are last-write-wins (07 §1 H-S3), so a save goes through `staleGuard`: it
// re-reads the values with `reload` and, when they differ from the copy the form started
// from, asks "Changed by someone else" (Reload / Overwrite). The window between that
// check and the write is accepted (P31).

import { CircleAlertIcon, RotateCcwIcon, TriangleAlertIcon } from "lucide-react";
import { type FormEvent, useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { ApiError } from "@/api/problem";
import type { SettingDefDto } from "@/api/themes";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { parseNumber } from "@/editor/fields/fields-model";
import { MediaChoice } from "@/editor/fields/media-field";
import { stableEqual, staleGuard } from "@/lib/stale-guard";
import { SecretField } from "./secret-field";
import { useStaleAsk } from "./use-stale-ask";

/** The kinds `SettingDefDto.type` names (`secret` only in a plugin's schema). */
const SETTING_KINDS = [
  "text",
  "textarea",
  "bool",
  "number",
  "select",
  "color",
  "media",
  "secret",
] as const;
export type SettingKind = (typeof SETTING_KINDS)[number];

const isKind = (type: string): type is SettingKind =>
  (SETTING_KINDS as readonly string[]).includes(type);

/** The core's limits per kind (`ferrite-core` `ext_settings`). */
const MAX_CHARS: Partial<Record<SettingKind, number>> = { text: 1_000, textarea: 65_536 };
/** What the form accepts for `color`; the server takes `#rgb` too. */
const HEX = /^#[0-9a-fA-F]{6}$/;

export type SettingValues = Record<string, unknown>;
export type SettingsPatch = Record<string, unknown>;

export interface SettingsFormProps {
  schema: SettingDefDto[];
  /** Every non-secret value as read (the default where nothing valid is stored). */
  values: SettingValues;
  /** Secret key → whether a value is set. */
  secrets?: Record<string, boolean>;
  /** Keys whose stored value does not fit the definition. */
  invalid?: string[];
  /** Re-reads the values for the stale check (`GET …/settings` → `values`). */
  reload: () => Promise<SettingValues>;
  /**
   * Sends the changed keys (`PATCH`); resolves to the values as saved, if the caller has
   * them, which the form then starts from. A rejected `ApiError` shows at its fields.
   */
  onSubmit: (patch: SettingsPatch) => Promise<SettingValues | undefined | void>;
  /** What the settings belong to, as the stale question names it ("Theme Blog"). */
  staleKey: string;
  /** Reports whether there are unsaved changes (for a leave guard). */
  onDirtyChange?: (dirty: boolean) => void;
  disabled?: boolean;
}

function without<T>(record: Record<string, T>, key: string): Record<string, T> {
  return Object.fromEntries(Object.entries(record).filter(([k]) => k !== key));
}

/** One edited field: a value, or reset to the default (sent as `null`). */
type Draft = { reset: false; value: unknown } | { reset: true };

export function SettingsForm({
  schema,
  values,
  secrets = {},
  invalid = [],
  reload,
  onSubmit,
  staleKey,
  onDirtyChange,
  disabled = false,
}: SettingsFormProps) {
  const { t } = useTranslation("admin");
  const stale = useStaleAsk(staleKey);
  // The copy the form started from (the stale check compares with it).
  const [base, setBase] = useState<SettingValues>(values);
  const [seen, setSeen] = useState<SettingValues>(values);
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [secretDrafts, setSecretDrafts] = useState<Record<string, string | null>>({});
  // Bumped to remount the secret fields (back to untouched) after a save or a reload.
  const [generation, setGeneration] = useState(0);
  const [serverErrors, setServerErrors] = useState<Record<string, string>>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const defs = schema.filter((d) => isKind(d.type));
  const kindOf = (d: SettingDefDto) => d.type as SettingKind;
  const effective = (d: SettingDefDto): unknown => {
    const draft = drafts[d.key];
    if (!draft) return base[d.key] ?? null;
    return draft.reset ? (d.default ?? null) : draft.value;
  };
  const changed = (d: SettingDefDto): boolean => {
    const draft = drafts[d.key];
    if (!draft) return false;
    return draft.reset || !stableEqual(draft.value, base[d.key] ?? null);
  };
  const clientError = (d: SettingDefDto): string | null => {
    if (!changed(d)) return null;
    const v = effective(d);
    if (kindOf(d) === "color" && (typeof v !== "string" || !HEX.test(v))) {
      return t("settingsForm.badColour");
    }
    if (kindOf(d) === "number" && typeof v === "string") return t("settingsForm.badNumber");
    return null;
  };

  const patch: SettingsPatch = {};
  for (const d of defs) {
    if (kindOf(d) === "secret") {
      const s = secretDrafts[d.key];
      if (s !== undefined) patch[d.key] = s;
    } else if (changed(d)) {
      const draft = drafts[d.key];
      const v = draft && !draft.reset ? draft.value : null;
      patch[d.key] = typeof v === "string" && kindOf(d) === "color" ? v.toLowerCase() : v;
    }
  }
  const dirty = Object.keys(patch).length > 0;
  const hasErrors = defs.some((d) => clientError(d) !== null);

  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);

  const startFrom = (next: SettingValues) => {
    setBase(next);
    setDrafts({});
    setSecretDrafts({});
    setServerErrors({});
    setGeneration((g) => g + 1);
  };

  // New values from the caller (a refetch) replace the start copy while nothing is
  // edited; with edits pending, the stale check at save time catches the difference.
  if (!stableEqual(values, seen)) {
    setSeen(values);
    if (!dirty) startFrom(values);
  }

  const edit = (key: string, draft: Draft) => {
    setDrafts((d) => ({ ...d, [key]: draft }));
    setServerErrors((e) => (key in e ? without(e, key) : e));
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (saving || disabled || !dirty || hasErrors) return;
    setSaving(true);
    setFailure(null);
    try {
      const guard = await staleGuard({ reread: reload, base, ask: stale.ask });
      if (!guard.proceed) {
        if (guard.reloaded) {
          startFrom(guard.reloaded);
          toast.info(t("settingsForm.reloaded"));
        }
        return;
      }
      const saved = await onSubmit(patch);
      if (saved) {
        setSeen(saved);
        startFrom(saved);
      } else {
        const next = { ...base };
        for (const d of defs) {
          if (kindOf(d) !== "secret" && d.key in patch) next[d.key] = effective(d);
        }
        startFrom(next);
      }
      toast.success(t("settingsForm.saved"));
    } catch (err) {
      reportApiError(err);
      const fields = err instanceof ApiError ? err.fieldErrors() : {};
      const known = Object.fromEntries(
        Object.entries(fields).filter(([key]) => defs.some((d) => d.key === key)),
      );
      setServerErrors(known);
      const message = err instanceof ApiError ? err.problem.detail || err.problem.title : null;
      if (Object.keys(known).length === 0) setFailure(message ?? t("settingsForm.notSaved"));
      toast.error(t("settingsForm.notSaved"), message ? { description: message } : {});
    } finally {
      setSaving(false);
    }
  };

  const unknown = schema.filter((d) => !isKind(d.type));
  const off = disabled || saving;

  return (
    <form
      data-slot="settings-form"
      noValidate
      onSubmit={(e) => void submit(e)}
      className="flex min-w-0 flex-col gap-6"
    >
      {failure ? (
        <p
          role="alert"
          className="flex items-start gap-2 rounded-lg border border-destructive/40 bg-destructive/5 px-3 py-2 text-sm text-destructive"
        >
          <CircleAlertIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0" />
          {failure}
        </p>
      ) : null}
      {defs.map((d) =>
        kindOf(d) === "secret" ? (
          <SecretSetting
            key={`${d.key}:${generation}`}
            def={d}
            set={secrets[d.key] === true}
            invalid={invalid.includes(d.key)}
            serverError={serverErrors[d.key] ?? null}
            disabled={off}
            onChange={(v) => {
              setSecretDrafts((s) => (v === undefined ? without(s, d.key) : { ...s, [d.key]: v }));
            }}
          />
        ) : (
          <Setting
            key={d.key}
            def={d}
            value={effective(d)}
            invalid={invalid.includes(d.key) && !drafts[d.key]}
            error={serverErrors[d.key] ?? clientError(d)}
            canReset={
              !drafts[d.key]?.reset &&
              (invalid.includes(d.key) || !stableEqual(effective(d), d.default ?? null))
            }
            disabled={off}
            onChange={(value) => {
              edit(d.key, { reset: false, value });
            }}
            onReset={() => {
              edit(d.key, { reset: true });
            }}
          />
        ),
      )}
      {unknown.map((d) => (
        <p key={d.key} className="text-sm text-muted-foreground">
          {t("settingsForm.unsupported", { label: d.label || d.key, kind: d.type })}
        </p>
      ))}
      <div className="flex flex-wrap items-center gap-3 border-t pt-4">
        <Button type="submit" disabled={off || !dirty}>
          {saving ? t("settingsForm.saving") : t("settingsForm.save")}
        </Button>
        {dirty && !saving ? (
          <span className="text-sm text-muted-foreground">
            {t("settingsForm.pending", { count: Object.keys(patch).length })}
          </span>
        ) : null}
      </div>
      {stale.dialog}
    </form>
  );
}

interface FieldIds {
  id: string;
  describedBy: string;
  invalid: boolean;
}

/** The label row (with Reset), the control, the default hint, the notices. */
function FieldFrame({
  def,
  canReset,
  onReset,
  disabled,
  invalid,
  error,
  hint,
  children,
}: {
  def: SettingDefDto;
  canReset: boolean;
  onReset?: () => void;
  disabled: boolean;
  invalid: boolean;
  error: string | null;
  hint: string | null;
  children: (ids: FieldIds) => React.ReactNode;
}) {
  const { t } = useTranslation("admin");
  const id = useId();
  const hintId = `${id}-hint`;
  const noticeId = `${id}-notice`;
  const errorId = `${id}-error`;
  const describedBy = [hint ? hintId : null, invalid ? noticeId : null, error ? errorId : null]
    .filter(Boolean)
    .join(" ");
  const label = def.label.trim() || def.key;
  return (
    <div
      data-slot="setting"
      data-key={def.key}
      data-kind={def.type}
      className="flex min-w-0 flex-col gap-2"
    >
      <div className="flex min-h-7 min-w-0 items-center justify-between gap-3">
        <Label htmlFor={id} className="min-w-0 wrap-anywhere">
          {label}
        </Label>
        {canReset && onReset ? (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={disabled}
            onClick={onReset}
            className="-mr-2 text-muted-foreground pointer-coarse:h-11"
          >
            <RotateCcwIcon data-icon="inline-start" />
            {t("settingsForm.reset")}
          </Button>
        ) : null}
      </div>
      {children({ id, describedBy, invalid: error !== null })}
      {hint ? (
        <p id={hintId} className="min-w-0 text-xs text-muted-foreground wrap-anywhere">
          {hint}
        </p>
      ) : null}
      {invalid ? (
        <p id={noticeId} className="flex items-start gap-1.5 text-sm text-warning">
          <TriangleAlertIcon aria-hidden="true" className="mt-0.5 size-3.5 shrink-0" />
          {t("settingsForm.invalid")}
        </p>
      ) : null}
      {error ? (
        <p id={errorId} data-slot="field-error" className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
    </div>
  );
}

/** The declared default as text ("on" / "off" for a switch); `null` when there is none. */
function defaultText(value: unknown, on: string, off: string): string | null {
  if (typeof value === "boolean") return value ? on : off;
  if (typeof value === "number") return String(value);
  if (typeof value === "string" && value !== "") return value;
  return null;
}

function Setting({
  def,
  value,
  invalid,
  error,
  canReset,
  disabled,
  onChange,
  onReset,
}: {
  def: SettingDefDto;
  value: unknown;
  invalid: boolean;
  error: string | null;
  canReset: boolean;
  disabled: boolean;
  onChange: (value: unknown) => void;
  onReset: () => void;
}) {
  const { t } = useTranslation(["admin", "editor"]);
  const kind = def.type as SettingKind;
  const dflt = defaultText(def.default, t("settingsForm.on"), t("settingsForm.off"));
  return (
    <FieldFrame
      def={def}
      canReset={canReset}
      onReset={onReset}
      disabled={disabled}
      invalid={invalid}
      error={error}
      hint={dflt === null ? null : t("settingsForm.default", { value: dflt })}
    >
      {(ids) => {
        const common = {
          id: ids.id,
          ...(ids.describedBy ? { "aria-describedby": ids.describedBy } : {}),
          ...(ids.invalid ? { "aria-invalid": true } : {}),
        };
        const text = typeof value === "string" ? value : "";
        switch (kind) {
          case "text":
            return (
              <Input
                {...common}
                disabled={disabled}
                maxLength={MAX_CHARS.text}
                value={text}
                onChange={(e) => onChange(e.target.value)}
              />
            );
          case "textarea":
            return (
              <Textarea
                {...common}
                disabled={disabled}
                maxLength={MAX_CHARS.textarea}
                rows={4}
                value={text}
                onChange={(e) => onChange(e.target.value)}
              />
            );
          case "bool":
            return (
              <div className="flex items-center">
                <Switch
                  {...common}
                  disabled={disabled}
                  checked={value === true}
                  onCheckedChange={(on) => onChange(on)}
                />
              </div>
            );
          case "number":
            return (
              <NumberSetting
                common={common}
                value={value}
                onChange={onChange}
                disabled={disabled}
              />
            );
          case "select": {
            // A value the options no longer have is still shown (and refused on save).
            const extra = text && !def.options.includes(text) ? [text] : [];
            return (
              <Select value={text} disabled={disabled} onValueChange={(v) => onChange(v)}>
                <SelectTrigger {...common} className="w-full max-w-sm">
                  <SelectValue placeholder={t("editor:fields.selectPlaceholder")} />
                </SelectTrigger>
                <SelectContent>
                  {[...extra, ...def.options].map((o) => (
                    <SelectItem key={o} value={o}>
                      {o}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            );
          }
          case "color":
            return (
              <ColourSetting common={common} value={text} onChange={onChange} disabled={disabled} />
            );
          case "media":
            return (
              <MediaChoice
                value={typeof value === "string" ? value : null}
                onChange={onChange}
                pickTitle={t("editor:fields.pickFile")}
                chooseLabel={t("editor:fields.chooseFile")}
                disabled={disabled}
                ids={{
                  id: ids.id,
                  describedBy: ids.describedBy,
                  invalid: ids.invalid,
                }}
              />
            );
          case "secret":
            return null;
        }
      }}
    </FieldFrame>
  );
}

type Common = { id: string; "aria-describedby"?: string; "aria-invalid"?: boolean };

function NumberSetting({
  common,
  value,
  onChange,
  disabled,
}: {
  common: Common;
  value: unknown;
  onChange: (v: unknown) => void;
  disabled: boolean;
}) {
  const shown = (v: unknown) =>
    typeof v === "number" ? String(v) : typeof v === "string" ? v : "";
  // The text as typed; a value set elsewhere (a reset, a reload) replaces it.
  const [text, setText] = useState(() => shown(value));
  const [seen, setSeen] = useState(value);
  if (!Object.is(seen, value)) {
    setSeen(value);
    if (!Object.is(parseNumber(text), value)) setText(shown(value));
  }
  return (
    <Input
      {...common}
      inputMode="decimal"
      autoComplete="off"
      disabled={disabled}
      value={text}
      onChange={(e) => {
        setText(e.target.value);
        const next = parseNumber(e.target.value);
        setSeen(next);
        onChange(next);
      }}
      className="w-full max-w-48 font-mono tabular-nums"
    />
  );
}

function ColourSetting({
  common,
  value,
  onChange,
  disabled,
}: {
  common: Common;
  value: string;
  onChange: (v: unknown) => void;
  disabled: boolean;
}) {
  const { t } = useTranslation("admin");
  // The swatch needs `#rrggbb`: a stored `#rgb` is expanded, anything else shows the last
  // good colour.
  const short = /^#([0-9a-fA-F])([0-9a-fA-F])([0-9a-fA-F])$/.exec(value);
  const swatch = HEX.test(value)
    ? value.toLowerCase()
    : short
      ? `#${short[1]}${short[1]}${short[2]}${short[2]}${short[3]}${short[3]}`.toLowerCase()
      : null;
  const [lastGood, setLastGood] = useState(swatch);
  if (swatch !== null && swatch !== lastGood) setLastGood(swatch);
  return (
    <div className="flex min-w-0 items-center gap-2">
      <input
        type="color"
        aria-label={t("settingsForm.pickColour")}
        disabled={disabled}
        value={lastGood ?? ""}
        onChange={(e) => onChange(e.target.value)}
        className="size-(--control-h) shrink-0 cursor-pointer rounded-md border border-input bg-background p-1 disabled:cursor-not-allowed disabled:opacity-50"
      />
      <Input
        {...common}
        disabled={disabled}
        spellCheck={false}
        autoComplete="off"
        maxLength={7}
        value={value}
        onChange={(e) => onChange(e.target.value.trim())}
        className="w-full max-w-36 font-mono"
      />
    </div>
  );
}

function SecretSetting({
  def,
  set,
  invalid,
  serverError,
  disabled,
  onChange,
}: {
  def: SettingDefDto;
  set: boolean;
  invalid: boolean;
  serverError: string | null;
  disabled: boolean;
  onChange: (v: string | null | undefined) => void;
}) {
  return (
    <FieldFrame
      def={def}
      canReset={false}
      disabled={disabled}
      invalid={invalid}
      error={serverError}
      hint={null}
    >
      {(ids) => (
        <SecretField
          id={ids.id}
          name={def.label.trim() || def.key}
          set={set}
          onChange={onChange}
          describedBy={ids.describedBy || undefined}
          invalid={ids.invalid}
          disabled={disabled}
        />
      )}
    </FieldFrame>
  );
}
