// The second step of a TOTP sign-in (`login_.totp.tsx`: un-nested from the login
// screen, pre-flight F20). The challenge comes in history state; the server answers a
// wrong or expired code with `401` (no `422`, F21), and after the challenge's 5 minutes
// the screen goes back to login with a notice.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  Link,
  createFileRoute,
  redirect,
  useNavigate,
  useRouterState,
} from "@tanstack/react-router";
import { ArrowLeftIcon, ShieldCheckIcon } from "lucide-react";
import { cn } from "cn";
import {
  type FormEvent,
  type Ref,
  useCallback,
  useEffect,
  useEffectEvent,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { loginTotp } from "@/api/auth";
import { isProblem } from "@/api/problem";
import type { SchemaLoginResponse } from "@/api/schema";
import { AuthLayout, FormAlert, SubmitButton } from "@/auth/auth-layout";
import { type FormFailure, genericFailure } from "@/auth/errors";
import { nextSearch, routerHref, safeNext } from "@/auth/next";
import { startSession } from "@/auth/session";

/** The challenge's lifetime (07 §2). */
const CHALLENGE_MS = 5 * 60_000;
const DIGITS = 6;

export const Route = createFileRoute("/login_/totp")({
  validateSearch: nextSearch,
  beforeLoad: ({ location, search }) => {
    const { totpChallenge, totpIssuedAt } = location.state;
    if (!totpChallenge || totpIssuedAt === undefined) {
      throw redirect({ to: "/login", search, replace: true });
    }
    if (Date.now() - totpIssuedAt >= CHALLENGE_MS) {
      throw redirect({
        to: "/login",
        search,
        replace: true,
        state: { authNotice: "totp-expired" },
      });
    }
  },
  component: TotpPage,
});

function TotpPage() {
  const { t } = useTranslation("auth");
  const { next } = Route.useSearch();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const challenge = useRouterState({ select: (s) => s.location.state.totpChallenge }) ?? "";
  const issuedAt = useRouterState({ select: (s) => s.location.state.totpIssuedAt }) ?? 0;
  const [code, setCode] = useState("");
  const [failure, setFailure] = useState<FormFailure | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const clearFailure = useCallback(() => {
    setFailure(null);
  }, []);

  // The challenge dies after 5 minutes: back to login, with a notice, at that moment.
  const expire = useEffectEvent(() => {
    void navigate({
      to: "/login",
      search: next ? { next } : {},
      replace: true,
      state: { authNotice: "totp-expired" },
    });
  });
  useEffect(() => {
    const id = setTimeout(expire, Math.max(0, issuedAt + CHALLENGE_MS - Date.now()));
    return () => {
      clearTimeout(id);
    };
  }, [issuedAt]);

  const verify = useMutation({
    mutationFn: loginTotp,
    onSuccess: async (r: SchemaLoginResponse) => {
      if (r.status !== "session") return;
      startSession(queryClient, r.user);
      if (r.user.must_reset) {
        await navigate({ to: "/change-password", search: next ? { next } : {}, replace: true });
      } else {
        await navigate({ href: routerHref(safeNext(next)), replace: true });
      }
    },
    onError: (e) => {
      setFailure(
        isProblem(e, "invalid-credentials")
          ? { kind: "message", key: "totp.invalid" }
          : genericFailure(e),
      );
      setCode("");
      inputRef.current?.focus();
    },
  });

  const locked = failure?.kind === "rate-limited" && failure.seconds !== null;
  const send = (value: string) => {
    if (locked || verify.isPending || value.length !== DIGITS) return;
    setFailure(null);
    verify.mutate({ challenge, code: value });
  };
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (code.length !== DIGITS) {
      inputRef.current?.focus();
      return;
    }
    send(code);
  };

  return (
    <AuthLayout
      title={t("totp.title")}
      description={t("totp.subtitle")}
      icon={<ShieldCheckIcon aria-hidden="true" />}
      footer={
        <Link
          to="/login"
          search={next ? { next } : {}}
          className="inline-flex min-h-11 items-center gap-1.5 self-start rounded-sm text-sm text-primary underline-offset-4 hover:underline"
        >
          <ArrowLeftIcon aria-hidden="true" className="size-3.5" />
          {t("totp.back")}
        </Link>
      }
    >
      <form className="flex flex-col gap-5" onSubmit={submit} noValidate>
        <FormAlert failure={failure} onExpired={clearFailure} />
        <div className="flex flex-col gap-2">
          <label htmlFor="totp-code" className="text-sm leading-none font-medium">
            {t("totp.code")}
          </label>
          <CodeInput
            ref={inputRef}
            value={code}
            invalid={failure?.kind === "message"}
            disabled={locked}
            onChange={(value) => {
              setCode(value);
              // Auto-submit once the sixth digit is in.
              if (value.length === DIGITS) send(value);
            }}
          />
          <p id="totp-code-hint" className="text-sm text-muted-foreground">
            {t("totp.codeHint")}
          </p>
        </div>
        <SubmitButton pending={verify.isPending} pendingLabel={t("totp.pending")} disabled={locked}>
          {t("totp.submit")}
        </SubmitButton>
      </form>
    </AuthLayout>
  );
}

/**
 * One real input (`inputmode="numeric"`, `autocomplete="one-time-code"`, so SMS and
 * password-manager autofill work) laid over six drawn cells in two groups of three.
 */
function CodeInput({
  ref,
  value,
  onChange,
  invalid,
  disabled,
}: {
  ref: Ref<HTMLInputElement>;
  value: string;
  onChange: (value: string) => void;
  invalid: boolean;
  disabled: boolean;
}) {
  const [focused, setFocused] = useState(false);
  const cells = Array.from({ length: DIGITS }, (_, i) => value[i] ?? "");
  const active = Math.min(value.length, DIGITS - 1);
  const cell = (i: number) => (
    <span
      key={i}
      className={cn(
        "flex h-12 w-11 items-center justify-center rounded-lg border bg-background font-mono text-xl font-medium text-foreground tabular-nums transition-colors duration-(--dur-instant) dark:bg-input/30",
        invalid ? "border-destructive" : "border-input",
        focused && i === active && !invalid && "border-primary ring-2 ring-primary/25",
      )}
    >
      {cells[i] ? (
        cells[i]
      ) : focused && i === active ? (
        <span className="h-6 w-px bg-foreground" />
      ) : null}
    </span>
  );
  return (
    <div className="relative w-full rounded-xl has-[input:focus-visible]:outline-2 has-[input:focus-visible]:outline-offset-4 has-[input:focus-visible]:outline-ring">
      <div aria-hidden="true" className="flex items-center justify-between">
        <div className="flex gap-2">{[0, 1, 2].map(cell)}</div>
        <span className="h-px w-3 bg-input" />
        <div className="flex gap-2">{[3, 4, 5].map(cell)}</div>
      </div>
      <input
        ref={ref}
        id="totp-code"
        name="code"
        type="text"
        inputMode="numeric"
        autoComplete="one-time-code"
        pattern="[0-9]*"
        maxLength={DIGITS}
        autoFocus
        disabled={disabled}
        aria-invalid={invalid || undefined}
        aria-describedby="totp-code-hint"
        value={value}
        onFocus={() => {
          setFocused(true);
        }}
        onBlur={() => {
          setFocused(false);
        }}
        onChange={(e) => {
          onChange(e.target.value.replace(/\D/g, "").slice(0, DIGITS));
        }}
        className="absolute inset-0 size-full cursor-text bg-transparent font-mono text-transparent caret-transparent opacity-0 outline-none selection:bg-transparent disabled:cursor-not-allowed"
      />
    </div>
  );
}
