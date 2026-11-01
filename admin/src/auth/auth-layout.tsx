// The frame of every sign-in screen: a centred card on a quiet ember-lit background,
// the product mark and the site's host (from `GET /meta`, pre-flight F22), and a way
// back to the public site. Below `sm` the card goes full-bleed.

import { useQuery } from "@tanstack/react-query";
import { ArrowLeftIcon, CircleAlertIcon, InfoIcon, TimerIcon } from "lucide-react";
import { cn } from "cn";
import type { ComponentProps, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { metaQuery } from "@/api/meta";
import { FerriteMark } from "@/components/kit/ferrite-mark";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { type FormFailure, useCountdown } from "./errors";

function hostOf(url: string): string | null {
  try {
    return new URL(url).host;
  } catch {
    return null;
  }
}

/** The site's host, or `null` while unknown (the meta call is public and cheap). */
function useSite(): { host: string | null; url: string | null; version: string | null } {
  const { data } = useQuery(metaQuery);
  if (!data) return { host: null, url: null, version: null };
  return { host: hostOf(data.site_url), url: data.site_url, version: data.version };
}

export interface AuthLayoutProps {
  title: string;
  description?: ReactNode;
  children: ReactNode;
  /** Under the card's content, above the card's edge (secondary links). */
  footer?: ReactNode;
  /** An icon badge above the title, for status screens (sent, done, expired). */
  icon?: ReactNode;
}

export function AuthLayout({ title, description, children, footer, icon }: AuthLayoutProps) {
  const { t } = useTranslation("auth");
  const site = useSite();
  return (
    <div className="relative isolate flex min-h-svh flex-col overflow-hidden bg-background">
      <Backdrop />
      <div className="flex flex-1 flex-col items-center px-4 pt-10 pb-8 sm:justify-center sm:px-6 sm:py-16">
        <main
          id="main"
          className="flex w-full max-w-[25rem] flex-col gap-6 duration-(--dur-base) ease-out animate-in fade-in slide-in-from-bottom-1"
        >
          <header className="flex items-center gap-3 px-1 sm:px-0">
            <FerriteMark />
            <div className="flex min-w-0 flex-col">
              <span className="text-sm leading-tight font-semibold text-foreground">
                {t("layout.mark")}
              </span>
              <span
                className="truncate font-mono text-xs text-muted-foreground"
                title={site.host ?? undefined}
              >
                {site.host ?? <span className="invisible">…</span>}
              </span>
            </div>
          </header>

          <section
            aria-labelledby="auth-title"
            className="flex flex-col gap-6 sm:rounded-2xl sm:border sm:bg-card sm:p-8 sm:shadow-2"
          >
            <div className="flex flex-col gap-2">
              {icon ? (
                <span className="mb-2 flex size-11 items-center justify-center rounded-xl border bg-card text-primary shadow-1 [&_svg]:size-5">
                  {icon}
                </span>
              ) : null}
              <h1 id="auth-title" className="text-2xl text-balance">
                {title}
              </h1>
              {description ? (
                <p className="text-md text-pretty text-muted-foreground sm:text-base">
                  {description}
                </p>
              ) : null}
            </div>
            {children}
            {footer ? (
              <div className="flex flex-col gap-3 border-t pt-5 text-sm text-muted-foreground">
                {footer}
              </div>
            ) : null}
          </section>
        </main>
      </div>
      <footer className="flex items-center justify-between gap-4 px-4 pb-4 text-xs text-muted-foreground sm:px-6 sm:pb-6">
        {site.url && site.host ? (
          <a
            href={site.url}
            title={t("layout.backToSite", { host: site.host })}
            className="inline-flex min-h-11 min-w-0 items-center gap-1.5 rounded-md hover:text-foreground"
          >
            <ArrowLeftIcon aria-hidden="true" className="size-3.5 shrink-0" />
            <span className="truncate">{t("layout.backToSite", { host: site.host })}</span>
          </a>
        ) : (
          <span />
        )}
        <span className="inline-flex shrink-0 items-center gap-2">
          <span className="max-sm:hidden">{t("layout.poweredBy")}</span>
          {site.version ? <span className="font-mono tabular-nums">v{site.version}</span> : null}
        </span>
      </footer>
    </div>
  );
}

/** Decoration only: an ember glow from the top and a faint dot grid that fades out. */
function Backdrop() {
  return (
    <div aria-hidden="true" className="pointer-events-none absolute inset-0 -z-10">
      <div className="absolute inset-0 bg-[radial-gradient(56rem_28rem_at_50%_-8rem,color-mix(in_oklch,var(--primary)_16%,transparent),transparent_72%)]" />
      <div className="absolute inset-0 bg-[radial-gradient(color-mix(in_oklch,var(--foreground)_10%,transparent)_1px,transparent_1.5px)] [mask-image:radial-gradient(ellipse_70%_55%_at_50%_0%,var(--foreground),transparent)] bg-size-[22px_22px]" />
    </div>
  );
}

// --- Form parts -------------------------------------------------------------------

export interface FieldProps extends Omit<ComponentProps<"input">, "id"> {
  id: string;
  label: string;
  /** Shown under the input; replaced by `error` when there is one. */
  hint?: ReactNode;
  error?: string | null;
  /** Right of the label (the "Forgot password?" link). */
  aside?: ReactNode;
}

/** A labelled text input at the auth screens' size (44 px; 16 px text on phones). */
export function Field({ id, label, hint, error, aside, className, ...props }: FieldProps) {
  const describedBy = error ? `${id}-error` : hint ? `${id}-hint` : undefined;
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-3">
        <Label htmlFor={id}>{label}</Label>
        {aside}
      </div>
      <Input
        id={id}
        aria-invalid={error ? true : undefined}
        aria-describedby={describedBy}
        className={cn(authInputClass, className)}
        {...props}
      />
      <FieldMessage id={id} hint={hint} error={error} />
    </div>
  );
}

export const authInputClass = "h-11 px-3 max-md:text-md";

export function FieldMessage({
  id,
  hint,
  error,
}: {
  id: string;
  hint?: ReactNode;
  error?: string | null | undefined;
}) {
  if (error) {
    return (
      <p id={`${id}-error`} className="flex items-start gap-1.5 text-sm text-destructive">
        <CircleAlertIcon aria-hidden="true" className="mt-0.5 size-3.5 shrink-0" />
        {error}
      </p>
    );
  }
  if (hint) {
    return (
      <p id={`${id}-hint`} className="text-sm text-muted-foreground">
        {hint}
      </p>
    );
  }
  return null;
}

/** The form's primary action: full width, 44 px, its label says when it is working. */
export function SubmitButton({
  pending,
  pendingLabel,
  children,
  disabled,
}: {
  pending: boolean;
  pendingLabel: string;
  children: ReactNode;
  disabled?: boolean;
}) {
  return (
    <Button
      type="submit"
      size="lg"
      className="h-11 w-full text-sm"
      disabled={disabled === true || pending}
      aria-busy={pending || undefined}
    >
      {pending ? pendingLabel : children}
    </Button>
  );
}

/** A neutral message above the form (signed out, sign-in cancelled). */
export function Notice({ children }: { children: ReactNode }) {
  return (
    <div
      role="status"
      className="flex items-start gap-2.5 rounded-lg border bg-muted/60 px-3 py-2.5 text-sm text-foreground"
    >
      <InfoIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-info" />
      <p>{children}</p>
    </div>
  );
}

/** The form-level failure: one message, or the rate-limit countdown. */
export function FormAlert({
  failure,
  onExpired,
}: {
  failure: FormFailure | null;
  /** Called when a rate limit's countdown reaches zero (the form unlocks). */
  onExpired: () => void;
}) {
  const { t } = useTranslation("auth");
  if (!failure) return null;
  if (failure.kind === "rate-limited") {
    return failure.seconds === null ? (
      <Alert tone="warning">{t("errors.rateLimitedLater")}</Alert>
    ) : (
      <RateLimitAlert key={failure.id} seconds={failure.seconds} onDone={onExpired} />
    );
  }
  return <Alert tone="error">{t(failure.key)}</Alert>;
}

function RateLimitAlert({ seconds, onDone }: { seconds: number; onDone: () => void }) {
  const { t } = useTranslation("auth");
  const left = useCountdown(seconds, onDone);
  return (
    <Alert tone="warning">
      {/* The visible text ticks; screen readers hear the start once. */}
      <span aria-hidden="true" data-testid="rate-limit-countdown" className="tabular-nums">
        {t("errors.rateLimited", { seconds: left })}
      </span>
      <span className="sr-only">{t("errors.rateLimitedSr", { seconds })}</span>
    </Alert>
  );
}

export function Alert({ tone, children }: { tone: "error" | "warning"; children: ReactNode }) {
  const Icon = tone === "error" ? CircleAlertIcon : TimerIcon;
  return (
    <div
      role="alert"
      data-tone={tone}
      className={cn(
        "flex items-start gap-2.5 rounded-lg border px-3 py-2.5 text-sm text-foreground",
        tone === "error"
          ? "border-destructive/40 bg-destructive/6"
          : "border-warning/50 bg-warning/8",
      )}
    >
      <Icon
        aria-hidden="true"
        className={cn(
          "mt-0.5 size-4 shrink-0",
          tone === "error" ? "text-destructive" : "text-warning",
        )}
      />
      <p className="min-w-0">{children}</p>
    </div>
  );
}
