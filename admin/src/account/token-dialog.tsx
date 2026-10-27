// A new API token (`POST /users/{me}/tokens`, H-A8): a name, the scopes as one row per
// area with None / Read / Write (only what this session holds is offered; `write`
// implies `read`), and an expiry (Never, 30, 90 or 365 days, sent as RFC 3339 UTC). The
// answer carries the bearer value once: it is fetched with a plain call (never a cached
// mutation) and shown with `CopyOnce` in the dialog body's state, which unmounts when
// the dialog closes (Review Focus 3).

import { cn } from "cn";
import { type FormEvent, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ApiError } from "@/api/problem";
import { type CreatedTokenDto, type ScopeArea, createToken } from "@/api/tokens";
import { FormAlert } from "@/auth/auth-layout";
import type { FormFailure } from "@/auth/errors";
import { CopyOnce } from "@/components/kit/copy-once";
import { Badge } from "@/components/ui/badge";
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
import { FormField } from "@/structure/types/form-field";
import {
  EXPIRY_CHOICES,
  type Expiry,
  expiresAt,
  offeredScopes,
  writeFailure,
} from "./account-model";

/** Core's limit on a token name, in characters. */
const MAX_TOKEN_NAME = 100;
type Level = "none" | "read" | "write";

export interface TokenDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  userId: string;
  /** The session's scopes (`me.scopes`). */
  held: readonly string[];
  /** A token was made (the list refreshes; the value stays in the dialog). */
  onCreated: () => void;
}

export function TokenDialog({ open, onOpenChange, userId, held, onCreated }: TokenDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="token-dialog" className="sm:max-w-xl">
        {/* Mounted only while open: the created value goes with it. */}
        <TokenBody
          userId={userId}
          held={held}
          onCreated={onCreated}
          close={() => onOpenChange(false)}
        />
      </DialogContent>
    </Dialog>
  );
}

interface BodyProps {
  userId: string;
  held: readonly string[];
  onCreated: () => void;
  close: () => void;
}

function TokenBody({ userId, held, onCreated, close }: BodyProps) {
  const { t } = useTranslation("admin");
  const offered = offeredScopes(held);
  const [name, setName] = useState("");
  const [levels, setLevels] = useState<Partial<Record<ScopeArea, Level>>>({});
  const [expiry, setExpiry] = useState<Expiry>("90");
  const [nameError, setNameError] = useState<string | null>(null);
  const [scopesError, setScopesError] = useState<string | null>(null);
  const [serverError, setServerError] = useState<string | null>(null);
  const [failure, setFailure] = useState<FormFailure | null>(null);
  const [pending, setPending] = useState(false);
  const [created, setCreated] = useState<CreatedTokenDto | null>(null);
  const nameInput = useRef<HTMLInputElement>(null);
  const ids = useId();
  const scopesLabelId = `${ids}-scopes`;
  const scopesHintId = `${ids}-scopes-hint`;
  const scopesErrorId = `${ids}-scopes-error`;
  const locked = failure?.kind === "rate-limited" && failure.seconds !== null;

  const scopes = offered.flatMap(({ area }) => {
    const level = levels[area] ?? "none";
    return level === "none" ? [] : [`${area}:${level}`];
  });

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending || locked) return;
    const trimmed = name.trim();
    const badName =
      trimmed === ""
        ? t("account.tokenDialog.nameRequired")
        : Array.from(trimmed).length > MAX_TOKEN_NAME
          ? t("account.tokenDialog.nameTooLong")
          : null;
    const badScopes = scopes.length === 0 ? t("account.tokenDialog.scopesRequired") : null;
    setNameError(badName);
    setScopesError(badScopes);
    if (badName) {
      nameInput.current?.focus();
      return;
    }
    if (badScopes) {
      document.querySelector<HTMLInputElement>(`[data-scope-grid="${ids}"] input`)?.focus();
      return;
    }
    setFailure(null);
    setServerError(null);
    setPending(true);
    try {
      const token = await createToken(userId, {
        name: trimmed,
        scopes,
        expires_at: expiresAt(expiry),
      });
      setCreated(token);
      onCreated();
    } catch (error) {
      if (error instanceof ApiError && error.status === 422) {
        const fields = error.fieldErrors();
        if (fields.name) setNameError(fields.name);
        if (fields.scopes) setScopesError(fields.scopes);
        // Anything else (the 50-token limit, an expiry refused): the server's words.
        if (!fields.name && !fields.scopes) {
          setServerError(
            fields.expires_at ?? error.problem.detail ?? t("account.tokenDialog.failed"),
          );
        }
      } else {
        setFailure(writeFailure(error));
      }
    } finally {
      setPending(false);
    }
  };

  if (created) {
    return (
      <div className="flex min-w-0 flex-col gap-5">
        <DialogHeader>
          <DialogTitle>{t("account.tokenDialog.createdTitle")}</DialogTitle>
          <DialogDescription>{t("account.tokenDialog.createdDescription")}</DialogDescription>
        </DialogHeader>
        <div className="flex min-w-0 flex-col gap-1.5">
          <span className="truncate text-sm font-medium text-foreground" title={created.name}>
            {created.name}
          </span>
          <ScopeChips scopes={created.scopes} />
        </div>
        <CopyOnce value={created.token} label={t("account.tokenDialog.token")} />
        <DialogFooter>
          <Button type="button" onClick={close} autoFocus>
            {t("account.tokenDialog.done")}
          </Button>
        </DialogFooter>
      </div>
    );
  }

  const setAll = (level: Level) => {
    setLevels(
      Object.fromEntries(
        offered.map(({ area, write }) => [area, level === "write" && !write ? "read" : level]),
      ),
    );
    setScopesError(null);
  };

  return (
    <form noValidate onSubmit={(e) => void submit(e)} className="flex min-w-0 flex-col gap-5">
      <DialogHeader>
        <DialogTitle>{t("account.tokenDialog.title")}</DialogTitle>
        <DialogDescription>{t("account.tokenDialog.description")}</DialogDescription>
      </DialogHeader>
      <FormAlert failure={failure} onExpired={() => setFailure(null)} />
      {serverError ? (
        <p
          role="alert"
          className="rounded-lg border border-destructive/40 bg-destructive/6 px-3 py-2.5 text-sm text-foreground"
        >
          {serverError}
        </p>
      ) : null}
      <FormField
        label={t("account.tokenDialog.name")}
        hint={t("account.tokenDialog.nameHint")}
        error={nameError ?? undefined}
      >
        {(control) => (
          <Input
            {...control}
            ref={nameInput}
            value={name}
            autoComplete="off"
            maxLength={MAX_TOKEN_NAME + 20}
            disabled={pending}
            autoFocus
            onChange={(e) => {
              setName(e.currentTarget.value);
              setNameError(null);
            }}
          />
        )}
      </FormField>

      <div className="flex min-w-0 flex-col gap-2">
        <div className="flex items-end justify-between gap-3">
          <div className="flex flex-col gap-0.5">
            <span id={scopesLabelId} className="text-sm font-medium text-foreground">
              {t("account.tokenDialog.scopes")}
            </span>
            <span id={scopesHintId} className="text-xs text-muted-foreground">
              {t("account.tokenDialog.scopesHint")}
            </span>
          </div>
          <div className="flex shrink-0 gap-1">
            <Button type="button" variant="ghost" size="sm" onClick={() => setAll("read")}>
              {t("account.tokenDialog.readAll")}
            </Button>
            <Button type="button" variant="ghost" size="sm" onClick={() => setAll("none")}>
              {t("account.tokenDialog.clear")}
            </Button>
          </div>
        </div>
        <div
          role="group"
          aria-labelledby={scopesLabelId}
          aria-describedby={scopesError ? `${scopesHintId} ${scopesErrorId}` : scopesHintId}
          data-scope-grid={ids}
          className={cn(
            "max-h-[min(22rem,40svh)] divide-y overflow-y-auto rounded-lg border",
            scopesError && "border-destructive",
          )}
        >
          {offered.map(({ area, write }) => (
            <ScopeRow
              key={area}
              name={`${ids}-${area}`}
              label={t(`account.tokenDialog.areas.${area}`)}
              write={write}
              value={levels[area] ?? "none"}
              disabled={pending}
              onChange={(level) => {
                setLevels((prev) => ({ ...prev, [area]: level }));
                setScopesError(null);
              }}
            />
          ))}
        </div>
        {scopesError ? (
          <p id={scopesErrorId} className="text-sm text-destructive">
            {scopesError}
          </p>
        ) : null}
      </div>

      <FormField label={t("account.tokenDialog.expires")} className="sm:max-w-56">
        {(control) => (
          <Select value={expiry} onValueChange={(v) => setExpiry(v as Expiry)} disabled={pending}>
            <SelectTrigger {...control} className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {EXPIRY_CHOICES.map((c) => (
                <SelectItem key={c} value={c}>
                  {t(`account.tokenDialog.expiry.${c}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      </FormField>

      <DialogFooter>
        <Button type="button" variant="outline" onClick={close}>
          {t("account.tokenDialog.cancel")}
        </Button>
        <Button type="submit" disabled={pending || locked} aria-busy={pending}>
          {pending ? t("account.tokenDialog.creating") : t("account.tokenDialog.create")}
        </Button>
      </DialogFooter>
    </form>
  );
}

function ScopeRow({
  name,
  label,
  write,
  value,
  disabled,
  onChange,
}: {
  name: string;
  label: string;
  write: boolean;
  value: Level;
  disabled: boolean;
  onChange: (level: Level) => void;
}) {
  const { t } = useTranslation("admin");
  const labelId = `${name}-label`;
  const levels: Level[] = write ? ["none", "read", "write"] : ["none", "read"];
  return (
    <div className="flex items-center justify-between gap-3 px-3 py-1.5">
      <span id={labelId} className="truncate text-sm text-foreground">
        {label}
      </span>
      <div
        role="radiogroup"
        aria-labelledby={labelId}
        className="inline-flex shrink-0 gap-0.5 rounded-lg border bg-muted/50 p-0.5"
      >
        {levels.map((level) => (
          <label
            key={level}
            className={cn(
              "relative flex h-[calc(var(--control-h)-6px)] min-w-14 cursor-pointer items-center justify-center rounded-md px-2.5 text-sm text-muted-foreground transition-colors duration-(--dur-instant) select-none",
              "hover:text-foreground has-checked:bg-background has-checked:text-foreground has-checked:shadow-1",
              level === "write" && "has-checked:text-primary",
              "has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-ring",
              "has-disabled:cursor-not-allowed has-disabled:opacity-60",
            )}
          >
            <input
              type="radio"
              name={name}
              value={level}
              checked={value === level}
              disabled={disabled}
              onChange={() => onChange(level)}
              className="absolute inset-0 size-full cursor-pointer appearance-none opacity-0 disabled:cursor-not-allowed"
            />
            {t(`account.tokenDialog.level.${level}`)}
          </label>
        ))}
      </div>
    </div>
  );
}

/** A token's scopes as small mono chips. */
export function ScopeChips({
  scopes,
  className,
}: {
  scopes: readonly string[];
  className?: string;
}) {
  return (
    <ul className={cn("flex min-w-0 flex-wrap gap-1", className)}>
      {scopes.map((s) => (
        <li key={s}>
          <Badge
            variant="secondary"
            className={cn("font-mono font-normal", s.endsWith(":write") && "text-foreground")}
          >
            {s}
          </Badge>
        </li>
      ))}
    </ul>
  );
}
