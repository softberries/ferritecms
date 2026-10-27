// One user, in a sheet over the users list (`?id=`; `manage_users`). For another user:
// the profile (display name, e-mail), role and status, Set password, Turn off two-factor,
// their API tokens (list and revoke; new tokens are made in Account only) and Delete.
// Accounts are last-write-wins (P11), so every edit goes through `staleGuard` (P31): the
// account is read again and, if it changed since the sheet opened, the admin chooses
// Reload or Overwrite. A role change or a disable asks first and says what it ends
// (H-A4); the server's refusals (the last admin, H-A2) show in that dialog.
// For one's own row (H-A1) role, status, password, two-factor and delete are not offered:
// the sheet links to Account instead, where the own e-mail needs the password (P44).

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { format } from "date-fns";
import { KeyRoundIcon, Trash2Icon, UserRoundCogIcon } from "lucide-react";
import { type FormEvent, type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { INLINE_ERRORS, reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { type TokenDto, tokensQuery } from "@/api/tokens";
import {
  type RoleDto,
  type UserDto,
  type UserStatus,
  disableUserTotp,
  getUser,
  patchUser,
  setUserPassword,
  userQuery,
} from "@/api/users";
import { withoutKey } from "@/account/account-model";
import { Expiry, RevokeDialog } from "@/account/tokens-card";
import { ScopeChips } from "@/account/token-dialog";
import { Alert } from "@/auth/auth-layout";
import { MAX_PASSWORD_BYTES, MIN_PASSWORD_CHARS, byteLength, charCount } from "@/auth/errors";
import { PasswordField } from "@/auth/password-field";
import type { MeDto } from "@/auth/session";
import { ErrorState } from "@/components/kit/error-state";
import { FormField } from "@/components/kit/form-field";
import { RelativeTime } from "@/components/kit/relative-time";
import { useStaleAsk } from "@/components/kit/use-stale-ask";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
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
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { DESTRUCTIVE } from "@/lib/destructive";
import { staleGuard } from "@/lib/stale-guard";
import { problemText } from "@/structure/taxonomies/terms-model";
import { DeleteUserDialog } from "./delete-user-dialog";
import { useRoleLabel } from "./role-label";
import { RoleBadge, TwoFactorBadge, UserAvatar, UserStatusBadge } from "./user-parts";
import { ROLES, STATUSES } from "./users-search";

/** Core's limit on a display name, in characters. */
const MAX_DISPLAY_NAME = 100;

export interface UserSheetProps {
  id: string;
  me: MeDto;
  onClose: () => void;
  onDeleted: (user: UserDto) => void;
}

export function UserSheet({ id, me, onClose, onDeleted }: UserSheetProps) {
  const { t } = useTranslation("admin");
  // Its error shows in the sheet (no toast on top).
  const user = useQuery({ ...userQuery(id), meta: INLINE_ERRORS });
  return (
    <Sheet open onOpenChange={(open) => !open && onClose()}>
      <SheetContent
        side="right"
        data-slot="user-sheet"
        className="w-full gap-0 overflow-y-auto p-0 data-[side=right]:w-full data-[side=right]:sm:max-w-lg"
      >
        {user.data ? (
          <SheetBody key={user.data.id} user={user.data} me={me} onDeleted={onDeleted} />
        ) : (
          <>
            <SheetHeader className="border-b p-5 pr-12">
              <SheetTitle>{t("users.sheet.loading")}</SheetTitle>
              <SheetDescription className="sr-only">
                {t("users.sheet.description")}
              </SheetDescription>
            </SheetHeader>
            <div className="p-5">
              {user.isError ? (
                <ErrorState error={user.error} retry={() => void user.refetch()} />
              ) : (
                <div aria-busy="true" className="flex flex-col gap-4">
                  <Skeleton className="h-16 w-full" />
                  <Skeleton className="h-9 w-full" />
                  <Skeleton className="h-9 w-full" />
                </div>
              )}
            </div>
          </>
        )}
      </SheetContent>
    </Sheet>
  );
}

/** The fields the sheet edits: what `staleGuard` compares (sign-ins do not count). */
const sameAccount = (a: UserDto, b: UserDto) =>
  a.display_name === b.display_name &&
  a.email === b.email &&
  a.role === b.role &&
  a.status === b.status;

type AccessChange = { kind: "role"; value: RoleDto } | { kind: "status"; value: UserStatus };

function SheetBody({
  user,
  me,
  onDeleted,
}: {
  user: UserDto;
  me: MeDto;
  onDeleted: (user: UserDto) => void;
}) {
  const { t } = useTranslation("admin");
  const roleLabel = useRoleLabel();
  const queryClient = useQueryClient();
  const own = user.id === me.id;
  // The account as the sheet last read or wrote it; the stale check compares with it.
  const [base, setBase] = useState(user);
  // Bumped when `base` is replaced from outside the profile form (a reload), so the form
  // starts again from it.
  const [generation, setGeneration] = useState(0);
  const [access, setAccess] = useState<AccessChange | null>(null);
  const [settingPassword, setSettingPassword] = useState(false);
  const [totpOff, setTotpOff] = useState(false);
  const [deleting, setDeleting] = useState<UserDto | null>(null);
  const stale = useStaleAsk(base.display_name);

  const adopt = (next: UserDto) => {
    setBase(next);
    queryClient.setQueryData(qk.user(next.id), next);
    void queryClient.invalidateQueries({ queryKey: qk.usersLists });
  };
  const reloaded = (theirs: UserDto) => {
    adopt(theirs);
    setGeneration((g) => g + 1);
    toast.info(t("users.sheet.reloaded"));
  };

  /**
   * The guarded write: `null` when the admin chose Reload or Cancel (nothing written),
   * else the saved account. Errors go to the caller.
   */
  const guardedPatch = async (body: Parameters<typeof patchUser>[1]): Promise<UserDto | null> => {
    const guard = await staleGuard({
      reread: () => getUser(base.id),
      base,
      equal: sameAccount,
      ask: stale.ask,
    });
    if (!guard.proceed) {
      if (guard.reloaded) reloaded(guard.reloaded);
      return null;
    }
    const saved = await patchUser(base.id, body);
    adopt(saved);
    return saved;
  };

  return (
    <div className="flex min-h-full flex-col">
      <SheetHeader className="gap-3 border-b p-5 pr-12">
        <div className="flex min-w-0 items-center gap-3">
          <UserAvatar user={base} size="lg" />
          <div className="flex min-w-0 flex-col gap-0.5">
            <SheetTitle className="truncate text-lg" title={base.display_name}>
              {base.display_name}
            </SheetTitle>
            <SheetDescription className="truncate font-mono text-xs" title={base.login}>
              {base.login}
            </SheetDescription>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <RoleBadge role={base.role} />
          <UserStatusBadge user={base} />
          <TwoFactorBadge on={base.totp_enabled} />
          {own ? <Badge variant="outline">{t("users.you")}</Badge> : null}
        </div>
      </SheetHeader>

      <div className="flex flex-col gap-6 p-5">
        {own ? (
          <div
            data-slot="own-row"
            className="flex flex-col gap-3 rounded-lg border bg-muted/50 p-4 text-sm"
          >
            <p className="text-foreground">{t("users.sheet.ownBody")}</p>
            <Button asChild variant="outline" className="self-start">
              <Link to="/account">
                <UserRoundCogIcon data-icon="inline-start" />
                {t("users.sheet.manageAccount")}
              </Link>
            </Button>
          </div>
        ) : (
          <ProfileForm
            key={generation}
            base={base}
            save={async (body) => {
              const saved = await guardedPatch(body);
              if (saved) toast.success(t("users.sheet.profileSaved"));
              return saved;
            }}
          />
        )}

        {own ? null : (
          <Section title={t("users.sheet.access")}>
            <div className="grid gap-4 sm:grid-cols-2">
              <FormField label={t("users.columns.role")}>
                {(control) => (
                  <Select
                    value={base.role}
                    onValueChange={(v) => {
                      const role = ROLES.find((r) => r === v);
                      if (role && role !== base.role) setAccess({ kind: "role", value: role });
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
              <FormField label={t("users.columns.status")}>
                {(control) => (
                  <Select
                    value={base.status}
                    onValueChange={(v) => {
                      const status = STATUSES.find((s) => s === v);
                      if (status && status !== base.status) {
                        setAccess({ kind: "status", value: status });
                      }
                    }}
                  >
                    <SelectTrigger {...control} className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent position="popper">
                      {STATUSES.map((s) => (
                        <SelectItem key={s} value={s}>
                          {t(`users.status.${s}`)}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              </FormField>
            </div>
            <p className="text-xs text-muted-foreground">{t("users.sheet.accessHint")}</p>
          </Section>
        )}

        {own ? null : (
          <Section title={t("users.sheet.signIn")}>
            <Row
              title={t("users.sheet.password")}
              body={
                base.must_reset ? t("users.sheet.passwordMustReset") : t("users.sheet.passwordBody")
              }
            >
              <Button variant="outline" onClick={() => setSettingPassword(true)}>
                {t("users.password.action")}
              </Button>
            </Row>
            <Row
              title={t("users.sheet.twoFactor")}
              body={
                base.totp_enabled ? t("users.sheet.twoFactorOn") : t("users.sheet.twoFactorOff")
              }
            >
              {base.totp_enabled ? (
                <Button variant="outline" onClick={() => setTotpOff(true)}>
                  {t("users.totp.action")}
                </Button>
              ) : null}
            </Row>
          </Section>
        )}

        <Section title={t("users.sheet.tokens")}>
          <UserTokens userId={base.id} />
        </Section>

        <dl className="grid grid-cols-2 gap-x-6 gap-y-3 rounded-lg bg-muted/60 px-4 py-3">
          <Fact label={t("users.sheet.created")}>
            <time
              dateTime={base.created_at}
              title={format(new Date(base.created_at), "PPpp")}
              className="tabular-nums"
            >
              {format(new Date(base.created_at), "PP")}
            </time>
          </Fact>
          <Fact label={t("users.columns.lastLogin")}>
            {base.last_login_at ? <RelativeTime at={base.last_login_at} /> : t("users.never")}
          </Fact>
        </dl>

        {own ? null : (
          <section
            aria-labelledby="user-danger"
            className="flex flex-col gap-3 rounded-lg border border-destructive/30 p-4"
          >
            <h3 id="user-danger" className="text-sm font-medium text-foreground">
              {t("users.sheet.danger")}
            </h3>
            <p className="text-sm text-muted-foreground">{t("users.sheet.dangerBody")}</p>
            <Button
              variant="outline"
              className="self-start text-destructive hover:text-destructive"
              onClick={() => setDeleting(base)}
            >
              <Trash2Icon data-icon="inline-start" />
              {t("users.delete.action")}
            </Button>
          </section>
        )}
      </div>

      <AccessDialog
        user={base}
        change={access}
        onClose={() => setAccess(null)}
        apply={async (change) => {
          const saved = await guardedPatch(
            change.kind === "role" ? { role: change.value } : { status: change.value },
          );
          if (saved) {
            toast.success(
              change.kind === "role"
                ? t("users.access.roleChanged", { role: roleLabel(saved.role) })
                : saved.status === "disabled"
                  ? t("users.access.disabled")
                  : t("users.access.enabled"),
            );
          }
          setAccess(null);
        }}
      />
      <SetPasswordDialog
        user={settingPassword ? base : null}
        onClose={() => setSettingPassword(false)}
        onDone={() => {
          setSettingPassword(false);
          // `must_reset` is now set.
          void queryClient.invalidateQueries({ queryKey: qk.user(base.id) });
          void queryClient.invalidateQueries({ queryKey: qk.usersLists });
          setBase((b) => ({ ...b, must_reset: true }));
        }}
      />
      <TotpOffDialog
        user={totpOff ? base : null}
        onClose={() => setTotpOff(false)}
        onDone={() => {
          setTotpOff(false);
          setBase((b) => ({ ...b, totp_enabled: false }));
          void queryClient.invalidateQueries({ queryKey: qk.user(base.id) });
          void queryClient.invalidateQueries({ queryKey: qk.usersLists });
        }}
      />
      <DeleteUserDialog
        user={deleting}
        onClose={() => setDeleting(null)}
        onDeleted={(gone) => {
          setDeleting(null);
          onDeleted(gone);
        }}
      />
      {stale.dialog}
    </div>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-3">
      <h3 className="text-sm font-medium text-foreground">{title}</h3>
      {children}
    </section>
  );
}

function Row({ title, body, children }: { title: string; body: string; children?: ReactNode }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border px-4 py-3">
      <div className="flex min-w-0 flex-1 basis-56 flex-col gap-0.5">
        <span className="text-sm font-medium text-foreground">{title}</span>
        <span className="text-sm text-muted-foreground">{body}</span>
      </div>
      {children}
    </div>
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

const PROFILE_FIELDS = ["display_name", "email"] as const;
type ProfileField = (typeof PROFILE_FIELDS)[number];

/** Display name and e-mail of another user (no password: P44 is only for one's own). */
function ProfileForm({
  base,
  save,
}: {
  base: UserDto;
  save: (body: { display_name?: string; email?: string }) => Promise<UserDto | null>;
}) {
  const { t } = useTranslation("admin");
  const [name, setName] = useState(base.display_name);
  const [email, setEmail] = useState(base.email);
  const [errors, setErrors] = useState<Partial<Record<ProfileField, string>>>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const nextName = name.trim();
  const nextEmail = email.trim();
  const nameChanged = nextName !== base.display_name;
  const emailChanged = nextEmail !== base.email;
  const dirty = nameChanged || emailChanged;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending || !dirty) return;
    const found: Partial<Record<ProfileField, string>> = {};
    if (nextName === "") found.display_name = t("users.form.nameRequired");
    else if (Array.from(nextName).length > MAX_DISPLAY_NAME) {
      found.display_name = t("users.form.nameTooLong");
    }
    if (!/^[^\s@]+@[^\s@]+$/.test(nextEmail)) found.email = t("users.form.emailInvalid");
    setErrors(found);
    setFailure(null);
    if (Object.keys(found).length > 0) return;
    setPending(true);
    try {
      const saved = await save({
        ...(nameChanged ? { display_name: nextName } : {}),
        ...(emailChanged ? { email: nextEmail } : {}),
      });
      if (saved) {
        setName(saved.display_name);
        setEmail(saved.email);
      }
    } catch (error) {
      reportApiError(error);
      const fields = error instanceof ApiError && error.status === 422 ? error.fieldErrors() : {};
      const known: Partial<Record<ProfileField, string>> = {};
      for (const f of PROFILE_FIELDS) if (fields[f]) known[f] = fields[f];
      if (Object.keys(known).length > 0) setErrors(known);
      else setFailure(problemText(error));
    } finally {
      setPending(false);
    }
  };

  return (
    <form noValidate onSubmit={(e) => void submit(e)} className="flex flex-col gap-4">
      <h3 className="text-sm font-medium text-foreground">{t("users.sheet.profile")}</h3>
      <FormField label={t("users.columns.displayName")} error={errors.display_name}>
        {(control) => (
          <Input
            {...control}
            value={name}
            autoComplete="off"
            disabled={pending}
            onChange={(e) => {
              setName(e.currentTarget.value);
              setErrors((errs) => withoutKey(errs, "display_name"));
            }}
          />
        )}
      </FormField>
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
              setErrors((errs) => withoutKey(errs, "email"));
            }}
          />
        )}
      </FormField>
      {failure ? <Alert tone="error">{failure}</Alert> : null}
      <Button
        type="submit"
        className="self-end"
        disabled={!dirty || pending}
        aria-busy={pending || undefined}
      >
        {pending ? t("users.sheet.saving") : t("users.sheet.saveProfile")}
      </Button>
    </form>
  );
}

/** The confirm of a role or status change; says what it ends (H-A4). */
function AccessDialog({
  user,
  change,
  onClose,
  apply,
}: {
  user: UserDto;
  change: AccessChange | null;
  onClose: () => void;
  apply: (change: AccessChange) => Promise<void>;
}) {
  const { t } = useTranslation("admin");
  const roleLabel = useRoleLabel();
  const [shown, setShown] = useState(change);
  if (change && change !== shown) setShown(change);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [opened, setOpened] = useState(change);
  if (change !== opened) {
    setOpened(change);
    setFailure(null);
  }
  const name = user.display_name;
  let title = "";
  let body = "";
  let confirm = "";
  let destructive = false;
  if (shown?.kind === "role") {
    title = t("users.access.roleTitle", { name, role: roleLabel(shown.value) });
    body = t("users.access.roleBody");
    confirm = t("users.access.roleConfirm");
  } else if (shown?.value === "disabled") {
    title = t("users.access.disableTitle", { name });
    body = t("users.access.disableBody");
    confirm = t("users.access.disableConfirm");
    destructive = true;
  } else if (shown) {
    title = t("users.access.enableTitle", { name });
    body = t("users.access.enableBody");
    confirm = t("users.access.enableConfirm");
  }

  const run = async () => {
    if (!change || pending) return;
    setPending(true);
    setFailure(null);
    try {
      await apply(change);
    } catch (error) {
      reportApiError(error);
      setFailure(problemText(error));
    } finally {
      setPending(false);
    }
  };

  return (
    <AlertDialog open={change !== null} onOpenChange={(open) => !open && !pending && onClose()}>
      <AlertDialogContent data-slot="access-change">
        <AlertDialogHeader>
          <AlertDialogTitle className="break-words">{title}</AlertDialogTitle>
          <AlertDialogDescription>{body}</AlertDialogDescription>
        </AlertDialogHeader>
        {failure ? <Alert tone="error">{failure}</Alert> : null}
        <AlertDialogFooter>
          <Button variant="outline" onClick={onClose} disabled={pending}>
            {t("users.cancel")}
          </Button>
          <Button
            className={destructive ? DESTRUCTIVE : undefined}
            onClick={() => void run()}
            disabled={pending}
            aria-busy={pending || undefined}
          >
            {confirm}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/**
 * `POST /users/{id}/password`: the typed password lives in this dialog's state only and
 * is sent with a plain call (never a mutation cache); it is not shown back.
 */
function SetPasswordDialog({
  user,
  onClose,
  onDone,
}: {
  user: UserDto | null;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation("admin");
  const [shown, setShown] = useState(user);
  if (user && user !== shown) setShown(user);
  return (
    <Dialog open={user !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent data-slot="set-password" className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="break-words">
            {t("users.password.title", { name: shown?.display_name ?? "" })}
          </DialogTitle>
          <DialogDescription>{t("users.password.body")}</DialogDescription>
        </DialogHeader>
        {user ? <SetPasswordForm user={user} onClose={onClose} onDone={onDone} /> : null}
      </DialogContent>
    </Dialog>
  );
}

function SetPasswordForm({
  user,
  onClose,
  onDone,
}: {
  user: UserDto;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation("admin");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending) return;
    if (charCount(password) < MIN_PASSWORD_CHARS) {
      setError(t("users.form.passwordShort", { min: MIN_PASSWORD_CHARS }));
      return;
    }
    if (byteLength(password) > MAX_PASSWORD_BYTES) {
      setError(t("users.form.passwordLong"));
      return;
    }
    setPending(true);
    setFailure(null);
    try {
      await setUserPassword(user.id, password);
      toast.success(t("users.password.done", { name: user.display_name }));
      onDone();
    } catch (err) {
      reportApiError(err);
      const fields = err instanceof ApiError && err.status === 422 ? err.fieldErrors() : {};
      if (fields.password) setError(fields.password);
      else setFailure(problemText(err));
    } finally {
      setPending(false);
    }
  };

  return (
    <form noValidate onSubmit={(e) => void submit(e)} className="flex flex-col gap-4">
      <PasswordField
        id="user-new-password"
        label={t("users.password.label")}
        value={password}
        onValueChange={(v) => {
          setPassword(v);
          setError(null);
        }}
        autoComplete="new-password"
        policy
        error={error}
        disabled={pending}
        autoFocus
      />
      {failure ? <Alert tone="error">{failure}</Alert> : null}
      <DialogFooter>
        <Button type="button" variant="outline" onClick={onClose} disabled={pending}>
          {t("users.cancel")}
        </Button>
        <Button type="submit" disabled={pending} aria-busy={pending || undefined}>
          {t("users.password.action")}
        </Button>
      </DialogFooter>
    </form>
  );
}

/** `POST /users/{id}/totp/disable` (P43), after a confirm. */
function TotpOffDialog({
  user,
  onClose,
  onDone,
}: {
  user: UserDto | null;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation("admin");
  const [shown, setShown] = useState(user);
  if (user && user !== shown) setShown(user);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  const run = async () => {
    if (!user || pending) return;
    setPending(true);
    setFailure(null);
    try {
      await disableUserTotp(user.id);
      toast.success(t("users.totp.done", { name: user.display_name }));
      onDone();
    } catch (error) {
      reportApiError(error);
      setFailure(problemText(error));
    } finally {
      setPending(false);
    }
  };

  return (
    <AlertDialog
      open={user !== null}
      onOpenChange={(open) => {
        if (!open && !pending) {
          setFailure(null);
          onClose();
        }
      }}
    >
      <AlertDialogContent data-slot="totp-off">
        <AlertDialogHeader>
          <AlertDialogTitle className="break-words">
            {t("users.totp.title", { name: shown?.display_name ?? "" })}
          </AlertDialogTitle>
          <AlertDialogDescription>{t("users.totp.body")}</AlertDialogDescription>
        </AlertDialogHeader>
        {failure ? <Alert tone="error">{failure}</Alert> : null}
        <AlertDialogFooter>
          <Button variant="outline" onClick={onClose} disabled={pending}>
            {t("users.cancel")}
          </Button>
          <Button
            className={DESTRUCTIVE}
            onClick={() => void run()}
            disabled={pending}
            aria-busy={pending || undefined}
          >
            {t("users.totp.confirm")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

function isExpired(token: TokenDto): boolean {
  return token.expires_at != null && new Date(token.expires_at).getTime() <= Date.now();
}

/** The user's API tokens, read-only but for Revoke (new ones are made in Account). */
function UserTokens({ userId }: { userId: string }) {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  const list = useQuery(tokensQuery(userId));
  const [revoking, setRevoking] = useState<TokenDto | null>(null);
  const tokens = list.data ?? [];

  let content: ReactNode;
  if (list.isError && !list.data) {
    content = <ErrorState error={list.error} retry={() => void list.refetch()} />;
  } else if (list.isPending) {
    content = (
      <div aria-busy="true" className="flex flex-col gap-2">
        <Skeleton className="h-12 w-full" />
      </div>
    );
  } else if (tokens.length === 0) {
    content = (
      <p className="flex items-center gap-2 rounded-lg border border-dashed px-4 py-3 text-sm text-muted-foreground">
        <KeyRoundIcon aria-hidden="true" className="size-4 shrink-0" />
        {t("users.sheet.noTokens")}
      </p>
    );
  } else {
    content = (
      <ul aria-label={t("users.sheet.tokens")} className="flex flex-col divide-y rounded-lg border">
        {tokens.map((token) => {
          const gone = isExpired(token);
          return (
            <li key={token.id} data-slot="user-token" className="flex items-start gap-3 px-4 py-3">
              <div className="flex min-w-0 flex-1 flex-col gap-1.5">
                <span className="flex min-w-0 items-center gap-2">
                  <span className="truncate text-sm font-medium text-foreground" title={token.name}>
                    {token.name}
                  </span>
                  {gone ? <Badge variant="destructive">{t("account.tokens.expired")}</Badge> : null}
                </span>
                <ScopeChips scopes={token.scopes} />
                <span className="flex flex-wrap gap-x-1 text-xs text-muted-foreground">
                  <span className="whitespace-nowrap">
                    {token.last_used_at ? (
                      <>
                        {t("users.sheet.tokenUsed")} <RelativeTime at={token.last_used_at} />
                      </>
                    ) : (
                      t("account.tokens.neverUsed")
                    )}
                  </span>
                  <span aria-hidden="true">·</span>
                  <span className="whitespace-nowrap">
                    <Expiry token={token} labelled />
                  </span>
                </span>
              </div>
              <Button
                variant="ghost"
                size="icon"
                aria-label={t("account.tokens.revokeLabel", { name: token.name })}
                title={t("account.tokens.revoke")}
                onClick={() => setRevoking(token)}
                className="shrink-0 text-muted-foreground hover:text-destructive"
              >
                <Trash2Icon />
              </Button>
            </li>
          );
        })}
      </ul>
    );
  }

  return (
    <>
      {content}
      <p className="text-xs text-muted-foreground">{t("users.sheet.tokensHint")}</p>
      <RevokeDialog
        token={revoking}
        userId={userId}
        onClose={() => setRevoking(null)}
        onRevoked={() => void queryClient.invalidateQueries({ queryKey: qk.tokens(userId) })}
      />
    </>
  );
}
