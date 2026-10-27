// New user (`POST /users`, `manage_users`): login, e-mail, display name and role, and how
// they get in. "Set a password" sends the typed password with `must_reset: true`, so they
// change it at their first sign-in; the password is never shown back. "Send a reset
// e-mail" (only when the server has mail set up, `me.mail_configured`) creates the
// account with `must_reset` and no password, then asks for a reset link for its login
// (`POST /auth/password-reset`). Both calls are plain awaited calls (the password never
// reaches a mutation cache) and the form lives only while the dialog is open.

import { type FormEvent, useState } from "react";
import { useTranslation } from "react-i18next";
import { reportApiError } from "@/api/client";
import { requestReset } from "@/api/auth";
import { ApiError } from "@/api/problem";
import { type RoleDto, type UserDto, createUser } from "@/api/users";
import { Alert } from "@/auth/auth-layout";
import { MAX_PASSWORD_BYTES, MIN_PASSWORD_CHARS, byteLength, charCount } from "@/auth/errors";
import { PasswordField } from "@/auth/password-field";
import type { MeDto } from "@/auth/session";
import { FormField } from "@/components/kit/form-field";
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
import { problemText } from "@/structure/taxonomies/terms-model";
import { useRoleLabel } from "./role-label";
import { ROLES } from "./users-search";

/** Core's `normalize_login` (after trimming and lower-casing). */
const LOGIN = /^[a-z0-9][a-z0-9._-]{0,59}$/;
const MAX_DISPLAY_NAME = 100;
const FIELDS = ["login", "email", "display_name", "role", "password"] as const;
type Field = (typeof FIELDS)[number];
type Access = "password" | "reset";

export interface NewUserDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  me: MeDto;
  /** The new account and what the toast says (the reset link went to their e-mail). */
  onCreated: (user: UserDto, note: string | null) => void;
}

export function NewUserDialog({ open, onOpenChange, me, onCreated }: NewUserDialogProps) {
  const { t } = useTranslation("admin");
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        data-slot="new-user"
        className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-lg"
      >
        <DialogHeader>
          <DialogTitle>{t("users.new.title")}</DialogTitle>
          <DialogDescription>{t("users.new.description")}</DialogDescription>
        </DialogHeader>
        {/* Mounted only while open: nothing typed outlives the dialog. */}
        {open ? (
          <NewUserForm
            mail={me.mail_configured === true}
            onCancel={() => onOpenChange(false)}
            onCreated={onCreated}
          />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function NewUserForm({
  mail,
  onCancel,
  onCreated,
}: {
  mail: boolean;
  onCancel: () => void;
  onCreated: (user: UserDto, note: string | null) => void;
}) {
  const { t } = useTranslation("admin");
  const roleLabel = useRoleLabel();
  const [login, setLogin] = useState("");
  const [email, setEmail] = useState("");
  const [name, setName] = useState("");
  const [role, setRole] = useState<RoleDto>("author");
  const [access, setAccess] = useState<Access>("password");
  const [password, setPassword] = useState("");
  const [errors, setErrors] = useState<Partial<Record<Field, string>>>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const clear = (field: Field) =>
    setErrors((errs) => {
      const next = { ...errs };
      delete next[field];
      return next;
    });

  const check = (): Partial<Record<Field, string>> => {
    const out: Partial<Record<Field, string>> = {};
    const l = login.trim().toLowerCase();
    if (l === "") out.login = t("users.form.loginRequired");
    else if (!LOGIN.test(l)) out.login = t("users.form.loginInvalid");
    if (!/^[^\s@]+@[^\s@]+$/.test(email.trim())) out.email = t("users.form.emailInvalid");
    const n = name.trim();
    if (n === "") out.display_name = t("users.form.nameRequired");
    else if (Array.from(n).length > MAX_DISPLAY_NAME) {
      out.display_name = t("users.form.nameTooLong");
    }
    if (access === "password") {
      if (charCount(password) < MIN_PASSWORD_CHARS) {
        out.password = t("users.form.passwordShort", { min: MIN_PASSWORD_CHARS });
      } else if (byteLength(password) > MAX_PASSWORD_BYTES) {
        out.password = t("users.form.passwordLong");
      }
    }
    return out;
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending) return;
    const found = check();
    setErrors(found);
    setFailure(null);
    if (Object.keys(found).length > 0) return;
    setPending(true);
    try {
      let created: UserDto;
      try {
        created = await createUser({
          login: login.trim(),
          email: email.trim(),
          display_name: name.trim(),
          role,
          ...(access === "password" ? { password } : {}),
          must_reset: true,
        });
      } catch (error) {
        reportApiError(error);
        const fields = error instanceof ApiError && error.status === 422 ? error.fieldErrors() : {};
        const known: Partial<Record<Field, string>> = {};
        for (const f of FIELDS) if (fields[f]) known[f] = fields[f];
        if (Object.keys(known).length > 0) setErrors(known);
        else setFailure(problemText(error));
        return;
      }
      if (access === "password") {
        onCreated(created, null);
        return;
      }
      try {
        await requestReset({ login: created.login });
        onCreated(created, t("users.new.resetSent", { email: created.email }));
      } catch (error) {
        // The account exists either way; say the mail did not go.
        reportApiError(error);
        onCreated(created, t("users.new.resetFailed", { detail: problemText(error) }));
      }
    } finally {
      setPending(false);
    }
  };

  return (
    <form noValidate onSubmit={(e) => void submit(e)} className="flex flex-col gap-5">
      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          label={t("users.form.login")}
          hint={t("users.form.loginHint")}
          error={errors.login}
        >
          {(control) => (
            <Input
              {...control}
              value={login}
              autoFocus
              autoComplete="off"
              autoCapitalize="none"
              autoCorrect="off"
              spellCheck={false}
              className="font-mono"
              disabled={pending}
              onChange={(e) => {
                setLogin(e.currentTarget.value);
                clear("login");
              }}
            />
          )}
        </FormField>
        <FormField label={t("users.columns.role")} error={errors.role}>
          {(control) => (
            <Select
              value={role}
              disabled={pending}
              onValueChange={(v) => {
                const r = ROLES.find((x) => x === v);
                if (r) setRole(r);
              }}
            >
              <SelectTrigger {...control} className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent position="popper">
                {ROLES.map((r) => (
                  <SelectItem key={r} value={r}>
                    {roleLabel(r)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        </FormField>
      </div>
      <FormField label={t("users.columns.email")} error={errors.email}>
        {(control) => (
          <Input
            {...control}
            type="email"
            value={email}
            autoComplete="off"
            spellCheck={false}
            disabled={pending}
            onChange={(e) => {
              setEmail(e.currentTarget.value);
              clear("email");
            }}
          />
        )}
      </FormField>
      <FormField label={t("users.columns.displayName")} error={errors.display_name}>
        {(control) => (
          <Input
            {...control}
            value={name}
            autoComplete="off"
            disabled={pending}
            onChange={(e) => {
              setName(e.currentTarget.value);
              clear("display_name");
            }}
          />
        )}
      </FormField>
      <fieldset className="flex flex-col gap-3">
        <legend className="mb-2 text-sm font-medium text-foreground">
          {t("users.new.accessLegend")}
        </legend>
        {mail ? (
          <div className="grid gap-2 sm:grid-cols-2">
            <AccessChoice
              value="password"
              checked={access === "password"}
              onChange={setAccess}
              label={t("users.new.setPassword")}
              disabled={pending}
            />
            <AccessChoice
              value="reset"
              checked={access === "reset"}
              onChange={setAccess}
              label={t("users.new.sendReset")}
              disabled={pending}
            />
          </div>
        ) : null}
        {access === "password" ? (
          <PasswordField
            id="new-user-password"
            label={t("users.new.password")}
            value={password}
            onValueChange={(v) => {
              setPassword(v);
              clear("password");
            }}
            autoComplete="new-password"
            policy
            error={errors.password ?? null}
            disabled={pending}
          />
        ) : null}
        <p className="text-xs text-muted-foreground">
          {access === "password" ? t("users.new.passwordHint") : t("users.new.resetHint")}
        </p>
      </fieldset>
      {failure ? <Alert tone="error">{failure}</Alert> : null}
      <DialogFooter>
        <Button type="button" variant="outline" onClick={onCancel} disabled={pending}>
          {t("users.cancel")}
        </Button>
        <Button type="submit" disabled={pending} aria-busy={pending || undefined}>
          {pending ? t("users.new.creating") : t("users.new.create")}
        </Button>
      </DialogFooter>
    </form>
  );
}

function AccessChoice({
  value,
  checked,
  onChange,
  label,
  disabled,
}: {
  value: Access;
  checked: boolean;
  onChange: (value: Access) => void;
  label: string;
  disabled: boolean;
}) {
  return (
    <label className="flex min-h-11 cursor-pointer items-center gap-3 rounded-lg border px-3 py-2 text-sm has-checked:border-primary/50 has-checked:bg-primary/8 has-focus-visible:ring-2 has-focus-visible:ring-ring">
      <input
        type="radio"
        name="new-user-access"
        value={value}
        checked={checked}
        disabled={disabled}
        onChange={() => onChange(value)}
        className="size-4 shrink-0 accent-primary focus-visible:outline-none"
      />
      {label}
    </label>
  );
}
