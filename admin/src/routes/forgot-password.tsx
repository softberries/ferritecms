// Ask for a password-reset e-mail. `202` is the same neutral answer whatever the
// account (07 §2), so the screen never says whether one exists.

import { useMutation } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import { ArrowLeftIcon, MailCheckIcon } from "lucide-react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useCallback, useState } from "react";
import { useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { requestReset } from "@/api/auth";
import { isProblem } from "@/api/problem";
import { AuthLayout, Field, FormAlert, SubmitButton } from "@/auth/auth-layout";
import { type FormFailure, genericFailure } from "@/auth/errors";
import { fieldMessage, forgotSchema } from "@/auth/schemas";

export const Route = createFileRoute("/forgot-password")({
  component: ForgotPasswordPage,
});

function ForgotPasswordPage() {
  const { t } = useTranslation("auth");
  const form = useForm({ resolver: zodResolver(forgotSchema), defaultValues: { login: "" } });
  const [failure, setFailure] = useState<FormFailure | null>(null);
  const clearFailure = useCallback(() => {
    setFailure(null);
  }, []);

  const request = useMutation({
    mutationFn: requestReset,
    onError: (e) => {
      setFailure(
        isProblem(e, "mail-unavailable")
          ? { kind: "message", key: "forgot.mailUnavailable" }
          : genericFailure(e),
      );
    },
  });

  const back = (
    <Link
      to="/login"
      className="inline-flex min-h-11 items-center gap-1.5 self-start rounded-sm text-sm text-primary underline-offset-4 hover:underline"
    >
      <ArrowLeftIcon aria-hidden="true" className="size-3.5" />
      {t("forgot.back")}
    </Link>
  );

  if (request.isSuccess) {
    return (
      <AuthLayout
        title={t("forgot.sentTitle")}
        description={t("forgot.sentBody")}
        icon={<MailCheckIcon aria-hidden="true" />}
        footer={back}
      >
        <p role="status" className="text-sm text-muted-foreground">
          {t("forgot.sentHint")}
        </p>
      </AuthLayout>
    );
  }

  const locked = failure?.kind === "rate-limited" && failure.seconds !== null;
  const submit = form.handleSubmit((values) => {
    if (locked || request.isPending) return;
    setFailure(null);
    request.mutate(values);
  });

  return (
    <AuthLayout title={t("forgot.title")} description={t("forgot.subtitle")} footer={back}>
      <form
        className="flex flex-col gap-5"
        onSubmit={(e) => {
          void submit(e);
        }}
        noValidate
      >
        <FormAlert failure={failure} onExpired={clearFailure} />
        <Field
          id="login"
          label={t("forgot.login")}
          autoComplete="username"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          autoFocus
          required
          error={fieldMessage(t, form.formState.errors.login)}
          {...form.register("login")}
        />
        <SubmitButton
          pending={request.isPending}
          pendingLabel={t("forgot.pending")}
          disabled={locked}
        >
          {t("forgot.submit")}
        </SubmitButton>
      </form>
    </AuthLayout>
  );
}
