// Choose a new password with the mailed link's token. `main.tsx` took the token from the
// fragment and dropped it from the address bar before anything rendered; it lives only
// in this route's loader data (memory) and in the confirm request's body (Review Focus 1).

import { useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import { CircleCheckIcon, LinkIcon } from "lucide-react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { confirmReset } from "@/api/auth";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { AuthLayout, FormAlert, SubmitButton } from "@/auth/auth-layout";
import { useSecretCall } from "@/auth/use-secret-call";
import { type FormFailure, genericFailure } from "@/auth/errors";
import { resetSchema } from "@/auth/schemas";
import { FormPasswordField } from "@/auth/password-field";
import { consumeResetToken } from "@/auth/reset-token";
import { buttonVariants } from "@/components/ui/button";

export const Route = createFileRoute("/reset-password")({
  // Runs once per visit: a later visit (or a reload) finds no token and says so.
  loader: () => ({ token: consumeResetToken() }),
  staleTime: Infinity,
  component: ResetPasswordPage,
});

const linkClass =
  "inline-flex min-h-11 items-center self-start rounded-sm text-sm text-primary underline-offset-4 hover:underline";

function ResetPasswordPage() {
  const { token } = Route.useLoaderData();
  const [expired, setExpired] = useState(token === null);
  const [done, setDone] = useState(false);
  if (done) return <Done />;
  if (expired || token === null) return <Expired />;
  return (
    <ResetForm
      token={token}
      onDone={() => {
        setDone(true);
      }}
      onExpired={() => {
        setExpired(true);
      }}
    />
  );
}

function ResetForm({
  token,
  onDone,
  onExpired,
}: {
  token: string;
  onDone: () => void;
  onExpired: () => void;
}) {
  const { t } = useTranslation("auth");
  const queryClient = useQueryClient();
  const form = useForm({
    resolver: zodResolver(resetSchema),
    defaultValues: { password: "", repeat: "" },
  });
  const [failure, setFailure] = useState<FormFailure | null>(null);

  const confirm = useSecretCall({
    mutationFn: confirmReset,
    onSuccess: () => {
      // Every session of the account ended; a cached user here is stale.
      queryClient.removeQueries({ queryKey: qk.me });
      onDone();
    },
    onError: (e) => {
      if (e instanceof ApiError && e.status === 422) {
        const fields = e.fieldErrors();
        if ("token" in fields) {
          onExpired();
          return;
        }
        form.setError(
          "password",
          fields.password
            ? { type: "server", message: fields.password }
            : { type: "validate", message: "policy.tooShort" },
          { shouldFocus: true },
        );
        return;
      }
      setFailure(genericFailure(e));
    },
  });

  const submit = form.handleSubmit(({ password }) => {
    if (confirm.isPending) return;
    setFailure(null);
    confirm.mutate({ token, password });
  });

  return (
    <AuthLayout title={t("reset.title")} description={t("reset.subtitle")}>
      <form
        className="flex flex-col gap-5"
        onSubmit={(e) => {
          void submit(e);
        }}
        noValidate
      >
        <FormAlert
          failure={failure}
          onExpired={() => {
            setFailure(null);
          }}
        />
        <FormPasswordField
          control={form.control}
          name="password"
          id="new-password"
          label={t("reset.new")}
          autoComplete="new-password"
          autoFocus
          required
          policy
        />
        <FormPasswordField
          control={form.control}
          name="repeat"
          id="repeat-password"
          label={t("reset.repeat")}
          autoComplete="new-password"
          required
        />
        <SubmitButton pending={confirm.isPending} pendingLabel={t("reset.pending")}>
          {t("reset.submit")}
        </SubmitButton>
      </form>
    </AuthLayout>
  );
}

function Done() {
  const { t } = useTranslation("auth");
  return (
    <AuthLayout
      title={t("reset.doneTitle")}
      description={t("reset.doneBody")}
      icon={<CircleCheckIcon aria-hidden="true" className="text-success" />}
    >
      <Link to="/login" className={buttonVariants({ size: "lg", className: "h-11 w-full" })}>
        {t("reset.signIn")}
      </Link>
    </AuthLayout>
  );
}

function Expired() {
  const { t } = useTranslation("auth");
  return (
    <AuthLayout
      title={t("reset.expiredTitle")}
      description={<span role="alert">{t("reset.expired")}</span>}
      icon={<LinkIcon aria-hidden="true" />}
      footer={
        <Link to="/login" className={linkClass}>
          {t("forgot.back")}
        </Link>
      }
    >
      <p className="text-sm text-muted-foreground">{t("reset.expiredBody")}</p>
      <Link
        to="/forgot-password"
        className={buttonVariants({ size: "lg", className: "h-11 w-full" })}
      >
        {t("reset.requestNew")}
      </Link>
    </AuthLayout>
  );
}
