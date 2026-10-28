// The redirect dialog (07 §3 Redirects; `manage_redirects`, the server checks again):
// From (a path), To (a path or an absolute http(s) URL) and the response code, checked
// here the way core checks them (`ferrite-core/src/redirects.rs`) so the common mistakes
// show at once; the server's own refusals (a router-owned source, R21/R26; a source whose
// trailing-slash variant is stored; a loop or a long chain) show on their field.
//
// - New: `POST /redirects` replaces the redirect stored for the same source, so the
//   source is looked up first (`findRedirect`); when one exists the form says so and the
//   button becomes Replace redirect.
// - Edit: `PATCH /redirects/{id}` through `staleGuard` (last-write-wins, Review Focus 4);
//   the re-read compares source, target and code only (hits move on every visit).

import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { reportApiError } from "@/api/client";
import { ApiError } from "@/api/problem";
import {
  type RedirectDto,
  findRedirect,
  getRedirect,
  patchRedirect,
  upsertRedirect,
} from "@/api/redirects";
import { useStaleAsk } from "@/components/kit/use-stale-ask";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { staleGuard } from "@/lib/stale-guard";
import { problemText } from "../taxonomies/terms-model";
import { without } from "../shared";
import { FormField } from "@/components/kit/form-field";
import { REDIRECT_CODES, type RedirectCode as Code, isRedirectCode } from "./codes";

/** Longest source or target core accepts, in characters. */
const MAX_URL = 2048;

type SourceProblem = "empty" | "long" | "path";
type TargetProblem = "empty" | "long" | "target";

/** Core's `check_source`, without the router-owned words (the server names those). */
function sourceProblem(source: string): SourceProblem | null {
  if (source === "") return "empty";
  if (Array.from(source).length > MAX_URL) return "long";
  if (!source.startsWith("/") || /[\s\p{Cc}]/u.test(source)) return "path";
  return null;
}

/** Core's `check_target`. */
function targetProblem(target: string): TargetProblem | null {
  if (target === "") return "empty";
  if (Array.from(target).length > MAX_URL) return "long";
  const lower = target.toLowerCase();
  const scheme = ["http://", "https://"].some(
    (p) => lower.startsWith(p) && lower.length > p.length,
  );
  const path = target.startsWith("/") && !target.startsWith("//");
  if (!(scheme || path) || /[\s\p{Cc}\\]/u.test(target)) return "target";
  return null;
}

const same = (a: RedirectDto, b: RedirectDto) =>
  a.source === b.source && a.target === b.target && a.code === b.code;

export type RedirectFormState = { kind: "new" } | { kind: "edit"; redirect: RedirectDto };

export function RedirectFormDialog({
  state,
  onClose,
  onSaved,
}: {
  /** `null`: closed. */
  state: RedirectFormState | null;
  onClose: () => void;
  onSaved: (saved: RedirectDto, how: "created" | "replaced" | "saved") => void;
}) {
  const { t } = useTranslation("structure");
  // A new key per opening, so the form starts from the redirect it was opened with; the
  // last state stays shown while the dialog closes.
  const [seen, setSeen] = useState<RedirectFormState | null>(null);
  const [opened, setOpened] = useState<{ state: RedirectFormState; key: number } | null>(null);
  if (state !== seen) {
    setSeen(state);
    if (state !== null) setOpened({ state, key: (opened?.key ?? 0) + 1 });
  }
  const shown = state ?? opened?.state ?? null;
  return (
    <Dialog
      open={state !== null}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent data-slot="redirect-form" className="sm:max-w-lg">
        {shown ? (
          <>
            <DialogHeader>
              <DialogTitle>
                {shown.kind === "edit"
                  ? t("redirects.form.editTitle")
                  : t("redirects.form.newTitle")}
              </DialogTitle>
              <DialogDescription>{t("redirects.form.description")}</DialogDescription>
            </DialogHeader>
            <RedirectForm key={opened?.key} state={shown} onClose={onClose} onSaved={onSaved} />
          </>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function RedirectForm({
  state,
  onClose,
  onSaved,
}: {
  state: RedirectFormState;
  onClose: () => void;
  onSaved: (saved: RedirectDto, how: "created" | "replaced" | "saved") => void;
}) {
  const { t } = useTranslation("structure");
  const [base, setBase] = useState<RedirectDto | null>(
    state.kind === "edit" ? state.redirect : null,
  );
  const [source, setSource] = useState(base?.source ?? "");
  const [target, setTarget] = useState(base?.target ?? "");
  const [code, setCode] = useState<Code>(base && isRedirectCode(base.code) ? base.code : 301);
  const [attempted, setAttempted] = useState(false);
  const [serverErrors, setServerErrors] = useState<Record<string, string>>({});
  const [failure, setFailure] = useState<string | null>(null);
  /** The stored redirect a new one would replace (shown, and confirmed by Replace). */
  const [exists, setExists] = useState<RedirectDto | null>(null);
  const [busy, setBusy] = useState(false);
  const stale = useStaleAsk(base?.source ?? "");

  const sp = sourceProblem(source);
  const tp = targetProblem(target);
  const sourceError =
    serverErrors.source ??
    (attempted && sp ? t(`redirects.form.errors.source.${sp}`, { max: MAX_URL }) : undefined);
  const targetError =
    serverErrors.target ??
    (attempted && tp ? t(`redirects.form.errors.target.${tp}`, { max: MAX_URL }) : undefined);
  const replacing = base === null && exists !== null && exists.source === source;

  const load = (dto: RedirectDto) => {
    setBase(dto);
    setSource(dto.source);
    setTarget(dto.target);
    setCode(isRedirectCode(dto.code) ? dto.code : 301);
    setAttempted(false);
    setServerErrors({});
    setFailure(null);
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    setAttempted(true);
    setFailure(null);
    if (sp || tp) return;
    setBusy(true);
    try {
      const body = { source, target, code };
      if (base === null) {
        if (!replacing) {
          const found = await findRedirect(source);
          if (found) {
            setExists(found);
            return;
          }
        }
        onSaved(await upsertRedirect(body), replacing ? "replaced" : "created");
      } else {
        const guard = await staleGuard({
          reread: () => getRedirect(base.id),
          base,
          equal: same,
          ask: stale.ask,
        });
        if (!guard.proceed) {
          if (guard.reloaded) load(guard.reloaded);
          return;
        }
        onSaved(await patchRedirect(base.id, body), "saved");
      }
    } catch (err) {
      reportApiError(err);
      const fields = err instanceof ApiError ? err.fieldErrors() : {};
      setServerErrors(fields);
      if (!fields.source && !fields.target && !fields.code) {
        setFailure(
          err instanceof ApiError && err.status === 404
            ? t("redirects.form.gone")
            : problemText(err),
        );
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <form noValidate onSubmit={(e) => void submit(e)} className="flex flex-col gap-5">
      <FormField
        label={t("redirects.form.source")}
        hint={t("redirects.form.sourceHint")}
        error={sourceError}
      >
        {(control) => (
          <Input
            {...control}
            autoFocus
            value={source}
            placeholder={t("redirects.form.sourcePlaceholder")}
            autoComplete="off"
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            className="font-mono"
            disabled={busy}
            onChange={(e) => {
              setSource(e.currentTarget.value.trim());
              setServerErrors((errs) => without(errs, "source"));
            }}
          />
        )}
      </FormField>
      <FormField
        label={t("redirects.form.target")}
        hint={t("redirects.form.targetHint")}
        error={targetError}
      >
        {(control) => (
          <Input
            {...control}
            value={target}
            placeholder={t("redirects.form.targetPlaceholder")}
            autoComplete="off"
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            className="font-mono"
            disabled={busy}
            onChange={(e) => {
              setTarget(e.currentTarget.value.trim());
              setServerErrors((errs) => without(errs, "target"));
            }}
          />
        )}
      </FormField>
      <FormField
        label={t("redirects.form.code")}
        hint={t(`redirects.codes.${code}.hint`)}
        error={serverErrors.code}
      >
        {(control) => (
          <Select
            value={String(code)}
            disabled={busy}
            onValueChange={(v) => {
              const n = Number(v);
              if (isRedirectCode(n)) setCode(n);
              setServerErrors((errs) => without(errs, "code"));
            }}
          >
            <SelectTrigger {...control} className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent position="popper">
              {REDIRECT_CODES.map((c) => (
                <SelectItem key={c} value={String(c)}>
                  <span className="font-mono tabular-nums">{c}</span>
                  <span>{t(`redirects.codes.${c}.label`)}</span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      </FormField>
      {replacing && exists ? (
        <div
          role="status"
          data-slot="redirect-exists"
          className="rounded-lg border border-warning/40 bg-warning/8 p-3 text-sm text-foreground"
        >
          <p className="break-all">
            {t("redirects.form.exists", {
              source: exists.source,
              target: exists.target,
              code: exists.code,
            })}
          </p>
          <p className="mt-1 text-muted-foreground">{t("redirects.form.existsHint")}</p>
        </div>
      ) : null}
      {failure ? (
        <p role="alert" className="text-sm text-destructive">
          {failure}
        </p>
      ) : null}
      <DialogFooter>
        <Button type="button" variant="outline" onClick={onClose}>
          {t("redirects.form.cancel")}
        </Button>
        <Button type="submit" disabled={busy}>
          {busy
            ? t("redirects.form.saving")
            : base
              ? t("redirects.form.save")
              : replacing
                ? t("redirects.form.replace")
                : t("redirects.form.create")}
        </Button>
      </DialogFooter>
      {stale.dialog}
    </form>
  );
}
