// Set another user's password (`POST /users/{id}/password`): they must change it at
// their next sign-in and their sessions end. The typed password lives in this dialog's
// form only (it mounts while open) and is sent with a plain call, never a mutation cache.

import { type FormEvent, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { ApiError } from "@/api/problem";
import { type UserDto, setUserPassword } from "@/api/users";
import { Alert } from "@/auth/auth-layout";
import { MIN_PASSWORD_CHARS } from "@/auth/errors";
import { PasswordField } from "@/auth/password-field";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { passwordProblem } from "@/lib/account-rules";
import { problemText } from "@/structure/taxonomies/terms-model";

/**
 * `POST /users/{id}/password`: the typed password lives in this dialog's state only and
 * is sent with a plain call (never a mutation cache); it is not shown back.
 */
export function SetPasswordDialog({
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
    const problem = passwordProblem(password);
    if (problem) {
      setError(
        problem === "short"
          ? t("users.form.passwordShort", { min: MIN_PASSWORD_CHARS })
          : t("users.form.passwordLong"),
      );
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
