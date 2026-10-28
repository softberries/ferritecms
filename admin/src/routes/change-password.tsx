// Change the password: forced for a `must_reset` session (the only screen it can reach,
// Review Focus 4), or by choice. The server rotates the session, so the new CSRF token
// is stored (`changePassword`), then the user goes on to `next`.

import { useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { KeyRoundIcon } from "lucide-react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useCallback, useState } from "react";
import { useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { changePassword } from "@/api/auth";
import { qk } from "@/api/keys";
import { ApiError, isProblem } from "@/api/problem";
import { AuthLayout, FormAlert, SubmitButton } from "@/auth/auth-layout";
import { useSecretCall } from "@/auth/use-secret-call";
import { type FormFailure, genericFailure } from "@/auth/errors";
import { changeSchema } from "@/auth/schemas";
import { nextSearch, routerHref, safeNext } from "@/auth/next";
import { FormPasswordField } from "@/auth/password-field";
import { type MeDto, loadSession, useLogout } from "@/auth/session";
import { Button } from "@/components/ui/button";

export const Route = createFileRoute("/change-password")({
  validateSearch: nextSearch,
  // A session is needed (a `must_reset` one included); without one, sign in first.
  beforeLoad: async (ctx) => ({ me: await loadSession(ctx) }),
  component: ChangePasswordPage,
});

function ChangePasswordPage() {
  const { t } = useTranslation("auth");
  const { me } = Route.useRouteContext();
  const { next } = Route.useSearch();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const logout = useLogout();
  const forced = me.must_reset;
  const form = useForm({
    resolver: zodResolver(changeSchema),
    defaultValues: { current: "", new: "" },
  });
  const [failure, setFailure] = useState<FormFailure | null>(null);
  const clearFailure = useCallback(() => {
    setFailure(null);
  }, []);

  const change = useSecretCall({
    mutationFn: changePassword,
    onSuccess: async (r) => {
      queryClient.setQueryData<MeDto>(qk.me, {
        ...me,
        must_reset: false,
        csrf_token: r.csrf_token ?? null,
      });
      await navigate({ href: routerHref(safeNext(next)), replace: true });
    },
    onError: (e) => {
      if (isProblem(e, "invalid-credentials")) {
        form.setError(
          "current",
          { type: "server", message: t("change.currentWrong") },
          {
            shouldFocus: true,
          },
        );
      } else if (e instanceof ApiError && e.status === 422) {
        const errors = e.fieldErrors();
        form.setError(
          "new",
          errors.new
            ? { type: "server", message: errors.new }
            : { type: "validate", message: "policy.tooShort" },
          { shouldFocus: true },
        );
      } else {
        setFailure(genericFailure(e));
      }
    },
  });

  const locked = failure?.kind === "rate-limited" && failure.seconds !== null;
  const submit = form.handleSubmit((values) => {
    if (locked || change.isPending) return;
    setFailure(null);
    change.mutate(values);
  });

  return (
    <AuthLayout
      title={forced ? t("change.titleForced") : t("change.title")}
      description={forced ? t("change.subtitleForced") : t("change.subtitle")}
      icon={<KeyRoundIcon aria-hidden="true" />}
      footer={
        forced ? (
          <Button
            variant="link"
            className="h-11 self-start px-0"
            onClick={() => {
              void logout();
            }}
          >
            {t("change.signOut")}
          </Button>
        ) : (
          <Link
            to="/"
            className="inline-flex min-h-11 items-center self-start rounded-sm text-sm text-primary underline-offset-4 hover:underline"
          >
            {t("change.cancel")}
          </Link>
        )
      }
    >
      <form
        className="flex flex-col gap-5"
        onSubmit={(e) => {
          void submit(e);
        }}
        noValidate
      >
        {/* For password managers: which account this new password belongs to. */}
        <input
          type="text"
          name="username"
          autoComplete="username"
          value={me.login}
          readOnly
          hidden
        />
        <FormAlert failure={failure} onExpired={clearFailure} />
        <FormPasswordField
          control={form.control}
          name="current"
          id="current-password"
          label={forced ? t("change.currentForced") : t("change.current")}
          autoComplete="current-password"
          autoFocus
          required
        />
        <FormPasswordField
          control={form.control}
          name="new"
          id="new-password"
          label={t("change.new")}
          autoComplete="new-password"
          required
          policy
        />
        <SubmitButton
          pending={change.isPending}
          pendingLabel={t("change.pending")}
          disabled={locked}
        >
          {t("change.submit")}
        </SubmitButton>
      </form>
    </AuthLayout>
  );
}
