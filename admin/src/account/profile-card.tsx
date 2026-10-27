// The profile: display name and e-mail (`PATCH /users/{me}`), with login, role and the
// account's dates read-only. A different e-mail asks for the current password (P44): the
// e-mail is where password resets go, so a session alone cannot move it. A wrong password
// (`403`) shows at that field and a rate limit (`429`, shared with password changes)
// shows its wait. After a save `qk.me` is refreshed (the user menu shows name and e-mail).

import { useQueryClient } from "@tanstack/react-query";
import { format } from "date-fns";
import { type FormEvent, type ReactNode, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { qk } from "@/api/keys";
import { type UserDto, patchUser } from "@/api/users";
import { FormAlert } from "@/auth/auth-layout";
import type { FormFailure } from "@/auth/errors";
import { PasswordField } from "@/auth/password-field";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardFooter } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { FormField } from "@/components/kit/form-field";
import { MAX_DISPLAY_NAME, displayNameProblem, isEmailShape } from "@/lib/account-rules";
import { fieldErrorsOf, isWrongPassword, withoutKey, writeFailure } from "./account-model";

const ROLES = ["admin", "editor", "author", "contributor"] as const;

const FIELDS = ["display_name", "email", "current_password"] as const;
type Field = (typeof FIELDS)[number];

export function ProfileCard({ user }: { user: UserDto }) {
  const { t } = useTranslation("admin");
  const { t: ts } = useTranslation("shell");
  const queryClient = useQueryClient();
  // What the form started from; a save moves it to the server's answer.
  const [base, setBase] = useState(user);
  const [name, setName] = useState(user.display_name);
  const [email, setEmail] = useState(user.email);
  const [password, setPassword] = useState("");
  const [errors, setErrors] = useState<Partial<Record<Field, string>>>({});
  const [failure, setFailure] = useState<FormFailure | null>(null);
  const [pending, setPending] = useState(false);

  const nextName = name.trim();
  const nextEmail = email.trim();
  const nameChanged = nextName !== base.display_name;
  const emailChanged = nextEmail !== base.email;
  const dirty = nameChanged || emailChanged;
  const locked = failure?.kind === "rate-limited" && failure.seconds !== null;
  const role = (ROLES as readonly string[]).includes(base.role)
    ? ts(`user.role.${base.role}`)
    : base.role;

  const check = (): Partial<Record<Field, string>> => {
    const out: Partial<Record<Field, string>> = {};
    const nameProblem = displayNameProblem(nextName);
    if (nameProblem === "required") out.display_name = t("account.profile.nameRequired");
    else if (nameProblem === "long") out.display_name = t("account.profile.nameTooLong");
    if (nextEmail === "") out.email = t("account.profile.emailRequired");
    else if (!isEmailShape(nextEmail)) out.email = t("account.profile.emailInvalid");
    if (emailChanged && password === "") {
      out.current_password = t("account.profile.passwordForEmail");
    }
    return out;
  };

  const inputs = useRef<Partial<Record<Field, HTMLInputElement | null>>>({});
  const focusFirst = (found: Partial<Record<Field, string>>) => {
    const first = FIELDS.find((f) => found[f]);
    if (first) inputs.current[first]?.focus();
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending || locked || !dirty) return;
    const found = check();
    setErrors(found);
    if (Object.keys(found).length > 0) {
      focusFirst(found);
      return;
    }
    setFailure(null);
    setPending(true);
    try {
      const saved = await patchUser(base.id, {
        ...(nameChanged ? { display_name: nextName } : {}),
        ...(emailChanged ? { email: nextEmail, current_password: password } : {}),
      });
      setBase(saved);
      setName(saved.display_name);
      setEmail(saved.email);
      setPassword("");
      queryClient.setQueryData(qk.user(saved.id), saved);
      void queryClient.invalidateQueries({ queryKey: qk.me });
      toast.success(t("account.profile.saved"));
    } catch (error) {
      if (isWrongPassword(error)) {
        setErrors({ current_password: t("account.wrongPassword") });
        focusFirst({ current_password: "x" });
        return;
      }
      const fields = fieldErrorsOf(error);
      const known = fields
        ? (Object.fromEntries(
            Object.entries(fields).filter(([k]) => (FIELDS as readonly string[]).includes(k)),
          ) as Partial<Record<Field, string>>)
        : {};
      if (Object.keys(known).length > 0) {
        setErrors(known);
        focusFirst(known);
        return;
      }
      setFailure(writeFailure(error));
    } finally {
      setPending(false);
    }
  };

  return (
    <Card>
      <form
        noValidate
        onSubmit={(e) => {
          void submit(e);
        }}
        className="flex flex-col gap-(--card-spacing)"
      >
        <CardContent className="flex flex-col gap-5">
          <dl
            data-slot="profile-facts"
            className="grid grid-cols-2 gap-x-6 gap-y-3 rounded-lg bg-muted/60 px-4 py-3 sm:grid-cols-4"
          >
            <Fact label={t("account.profile.login")}>
              <span className="truncate font-mono" title={base.login}>
                {base.login}
              </span>
            </Fact>
            <Fact label={t("account.profile.role")}>{role}</Fact>
            <Fact label={t("account.profile.memberSince")}>
              <time
                dateTime={base.created_at}
                title={format(new Date(base.created_at), "PPpp")}
                className="tabular-nums"
              >
                {format(new Date(base.created_at), "PP")}
              </time>
            </Fact>
            <Fact label={t("account.profile.lastSignIn")}>
              {base.last_login_at ? (
                <RelativeTime at={base.last_login_at} />
              ) : (
                t("account.profile.never")
              )}
            </Fact>
          </dl>
          <FormAlert failure={failure} onExpired={() => setFailure(null)} />
          <div className="grid gap-5 sm:grid-cols-2">
            <FormField label={t("account.profile.displayName")} error={errors.display_name}>
              {(control) => (
                <Input
                  {...control}
                  ref={(el) => {
                    inputs.current.display_name = el;
                  }}
                  value={name}
                  autoComplete="name"
                  maxLength={MAX_DISPLAY_NAME + 20}
                  disabled={pending}
                  onChange={(e) => {
                    setName(e.currentTarget.value);
                    setErrors((errs) => withoutKey(errs, "display_name"));
                  }}
                />
              )}
            </FormField>
            <FormField label={t("account.profile.email")} error={errors.email}>
              {(control) => (
                <Input
                  {...control}
                  ref={(el) => {
                    inputs.current.email = el;
                  }}
                  type="email"
                  value={email}
                  autoComplete="email"
                  spellCheck={false}
                  disabled={pending}
                  onChange={(e) => {
                    setEmail(e.currentTarget.value);
                    setErrors((errs) => withoutKey(errs, "email"));
                  }}
                />
              )}
            </FormField>
          </div>
          {emailChanged ? (
            <div
              data-slot="email-password"
              className="flex flex-col gap-3 rounded-lg border border-dashed px-4 py-3 duration-(--dur-base) animate-in fade-in"
            >
              <p className="text-sm text-muted-foreground">
                {t("account.profile.emailChangeHint")}
              </p>
              {/* For password managers: which account the password belongs to. */}
              <input
                type="text"
                name="username"
                autoComplete="username"
                value={base.login}
                readOnly
                hidden
              />
              <div className="sm:max-w-[calc(50%-0.625rem)]">
                <PasswordField
                  id="profile-current-password"
                  ref={(el) => {
                    inputs.current.current_password = el;
                  }}
                  label={t("account.profile.currentPassword")}
                  value={password}
                  onValueChange={(v) => {
                    setPassword(v);
                    setErrors((errs) => withoutKey(errs, "current_password"));
                  }}
                  autoComplete="current-password"
                  error={errors.current_password ?? null}
                  disabled={pending}
                />
              </div>
            </div>
          ) : null}
        </CardContent>
        <CardFooter className="justify-end border-t py-3">
          <Button type="submit" disabled={!dirty || pending || locked} aria-busy={pending}>
            {pending ? t("account.profile.saving") : t("account.profile.save")}
          </Button>
        </CardFooter>
      </form>
    </Card>
  );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="flex min-w-0 text-sm text-foreground">{children}</dd>
    </div>
  );
}
