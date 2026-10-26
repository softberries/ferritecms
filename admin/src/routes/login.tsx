// Sign in: login (or e-mail) and password. A TOTP account continues at `/login/totp`
// with the challenge in history state, never in the URL.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  Link,
  createFileRoute,
  redirect,
  useNavigate,
  useRouterState,
} from "@tanstack/react-router";
import { zodResolver } from "@hookform/resolvers/zod";
import { useCallback, useState } from "react";
import { useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { login } from "@/api/auth";
import { getCsrfToken } from "@/api/csrf";
import { qk } from "@/api/keys";
import { isProblem } from "@/api/problem";
import type { SchemaLoginResponse } from "@/api/schema";
import { AuthLayout, Field, FormAlert, Notice, SubmitButton } from "@/auth/auth-layout";
import { type FormFailure, genericFailure } from "@/auth/errors";
import { fieldMessage, loginSchema } from "@/auth/schemas";
import { nextSearch, routerHref, safeNext } from "@/auth/next";
import { FormPasswordField } from "@/auth/password-field";
import { type MeDto, startSession } from "@/auth/session";

export const Route = createFileRoute("/login")({
  validateSearch: nextSearch,
  beforeLoad: ({ context, search }) => {
    // Already signed in this page's lifetime: straight on.
    const user = context.queryClient.getQueryData<MeDto>(qk.me);
    if (user && !user.must_reset && getCsrfToken() !== null) {
      throw redirect({ href: routerHref(safeNext(search.next)), replace: true });
    }
  },
  component: LoginPage,
});

function LoginPage() {
  const { t } = useTranslation("auth");
  const { next } = Route.useSearch();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const notice = useRouterState({ select: (s) => s.location.state.authNotice });
  const form = useForm({
    resolver: zodResolver(loginSchema),
    defaultValues: { login: "", password: "" },
  });
  const { errors } = form.formState;
  const [failure, setFailure] = useState<FormFailure | null>(null);
  const clearFailure = useCallback(() => {
    setFailure(null);
  }, []);

  const signIn = useMutation({
    mutationFn: login,
    onSuccess: async (r: SchemaLoginResponse) => {
      if (r.status === "totp_required") {
        await navigate({
          to: "/login/totp",
          search: next ? { next } : {},
          state: { totpChallenge: r.challenge, totpIssuedAt: Date.now() },
        });
        return;
      }
      startSession(queryClient, r.user);
      if (r.user.must_reset) {
        await navigate({ to: "/change-password", search: next ? { next } : {}, replace: true });
      } else {
        await navigate({ href: routerHref(safeNext(next)), replace: true });
      }
    },
    onError: (e) => {
      if (isProblem(e, "invalid-credentials")) {
        setFailure({ kind: "message", key: "login.invalid" });
        form.resetField("password");
        form.setFocus("password");
      } else {
        setFailure(genericFailure(e));
      }
    },
  });

  const locked = failure?.kind === "rate-limited" && failure.seconds !== null;
  const submit = form.handleSubmit((values) => {
    if (locked || signIn.isPending) return;
    setFailure(null);
    signIn.mutate(values);
  });

  return (
    <AuthLayout title={t("login.title")} description={t("login.subtitle")}>
      <form
        className="flex flex-col gap-5"
        onSubmit={(e) => {
          void submit(e);
        }}
        noValidate
      >
        {notice && !failure ? (
          <Notice>
            {notice === "totp-expired" ? t("login.noticeTotpExpired") : t("login.noticeSignedOut")}
          </Notice>
        ) : null}
        <FormAlert failure={failure} onExpired={clearFailure} />
        <Field
          id="login"
          label={t("login.login")}
          autoComplete="username"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          autoFocus
          required
          error={fieldMessage(t, errors.login)}
          {...form.register("login")}
        />
        <FormPasswordField
          control={form.control}
          name="password"
          id="password"
          label={t("login.password")}
          autoComplete="current-password"
          required
          aside={
            <Link
              to="/forgot-password"
              className="rounded-sm text-sm text-primary underline-offset-4 hover:underline"
            >
              {t("login.forgot")}
            </Link>
          }
        />
        <SubmitButton
          pending={signIn.isPending}
          pendingLabel={t("login.pending")}
          disabled={locked}
        >
          {t("login.submit")}
        </SubmitButton>
      </form>
    </AuthLayout>
  );
}
