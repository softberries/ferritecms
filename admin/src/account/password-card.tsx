// Change the password (`POST /auth/password`): current, new and repeat, against the
// policy (`changeSchema`, 07 §2). The server rotates this session and ends every other
// one; `changePassword` stores the new CSRF token, and `qk.me` takes it too, so the next
// write of this tab still passes (Review Focus 1). API tokens are not affected (R22).

import { zodResolver } from "@hookform/resolvers/zod";
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { changePassword } from "@/api/auth";
import { qk } from "@/api/keys";
import { ApiError, isProblem } from "@/api/problem";
import { FormAlert } from "@/auth/auth-layout";
import type { FormFailure } from "@/auth/errors";
import { FormPasswordField } from "@/auth/password-field";
import type { MeDto } from "@/auth/session";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardFooter } from "@/components/ui/card";
import { passwordSchema, writeFailure } from "./account-model";

export function PasswordCard({ login }: { login: string }) {
  const { t } = useTranslation("admin");
  const { t: ta } = useTranslation("auth");
  const queryClient = useQueryClient();
  const form = useForm({
    resolver: zodResolver(passwordSchema),
    defaultValues: { current: "", new: "", repeat: "" },
  });
  const [failure, setFailure] = useState<FormFailure | null>(null);
  const [pending, setPending] = useState(false);
  const locked = failure?.kind === "rate-limited" && failure.seconds !== null;

  const submit = form.handleSubmit(async (values) => {
    if (pending || locked) return;
    setFailure(null);
    setPending(true);
    try {
      const changed = await changePassword({ current: values.current, new: values.new });
      queryClient.setQueryData<MeDto>(qk.me, (me) =>
        me ? { ...me, must_reset: false, csrf_token: changed.csrf_token ?? null } : me,
      );
      void queryClient.invalidateQueries({ queryKey: qk.sessions });
      form.reset();
      toast.success(t("account.password.changed"), {
        description: t("account.password.othersSignedOut"),
      });
    } catch (error) {
      if (isProblem(error, "invalid-credentials")) {
        form.setError(
          "current",
          { type: "server", message: ta("change.currentWrong") },
          { shouldFocus: true },
        );
      } else if (error instanceof ApiError && error.status === 422) {
        const detail = error.fieldErrors().new;
        form.setError(
          "new",
          detail
            ? { type: "server", message: detail }
            : { type: "validate", message: "policy.tooShort" },
          { shouldFocus: true },
        );
      } else {
        setFailure(writeFailure(error));
      }
    } finally {
      setPending(false);
    }
  });

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
          {/* For password managers: which account the new password belongs to. */}
          <input
            type="text"
            name="username"
            autoComplete="username"
            value={login}
            readOnly
            hidden
          />
          <FormAlert failure={failure} onExpired={() => setFailure(null)} />
          <div className="sm:max-w-[calc(50%-0.625rem)]">
            <FormPasswordField
              control={form.control}
              name="current"
              id="password-current"
              label={t("account.password.current")}
              autoComplete="current-password"
              disabled={pending}
              required
            />
          </div>
          <div className="grid items-start gap-5 sm:grid-cols-2">
            <FormPasswordField
              control={form.control}
              name="new"
              id="password-new"
              label={t("account.password.new")}
              autoComplete="new-password"
              disabled={pending}
              required
              policy
            />
            <FormPasswordField
              control={form.control}
              name="repeat"
              id="password-repeat"
              label={t("account.password.repeat")}
              autoComplete="new-password"
              disabled={pending}
              required
            />
          </div>
        </CardContent>
        <CardFooter className="justify-end border-t py-3">
          <Button type="submit" disabled={pending || locked} aria-busy={pending}>
            {pending ? t("account.password.pending") : t("account.password.submit")}
          </Button>
        </CardFooter>
      </form>
    </Card>
  );
}
