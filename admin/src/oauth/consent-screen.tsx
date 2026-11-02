// /admin/connect?request=<id> (12 §3.2, §8): where `/oauth/authorize` sends the browser.
// The signed-in user sees which app asks (its logo only when `https`, verified by its
// metadata document or self-registered), where the browser goes back to, and what it
// may do as one row per area (Off / Read / Write), preselected from `grantable` and
// never above it. Approve sends the chosen scopes, Deny refuses; each answers
// `redirect_to`, and the browser goes there with a full navigation (it may be a custom
// scheme or a loopback address). There is no `<form>`: Enter approves only on the
// Approve button itself, and Escape does nothing.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { cn } from "cn";
import {
  CircleCheckIcon,
  CircleSlashIcon,
  ShieldCheckIcon,
  TimerOffIcon,
  TriangleAlertIcon,
} from "lucide-react";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { reportApiError } from "@/api/client";
import { metaQuery } from "@/api/meta";
import { type ConsentDto, approveConsent, consentQuery, denyConsent } from "@/api/oauth";
import { ApiError } from "@/api/problem";
import type { ScopeArea } from "@/api/tokens";
import { Alert, AuthLayout } from "@/auth/auth-layout";
import { type MeDto, logout } from "@/auth/session";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { useRoleLabel } from "@/users/role-label";
import { ClientKind, ClientLogo } from "./client-mark";
import { listOf, safeRedirect } from "./model";
import {
  type Level,
  type Levels,
  areasOf,
  beyondRole,
  levelsOf,
  readOnly,
  scopesOf,
} from "./scope-levels";

/** The browser's full navigation to the client (replaced in tests). */
function assignLocation(url: string): void {
  window.location.assign(url);
}

export interface ConsentScreenProps {
  /** The `request` query parameter; absent when the page was opened by hand. */
  requestId: string | undefined;
  me: MeDto;
  /** Sends the browser to the client's redirect URI. */
  redirect?: (url: string) => void;
}

export function ConsentScreen({ requestId, me, redirect = assignLocation }: ConsentScreenProps) {
  const { t } = useTranslation("ai");
  const consent = useQuery({ ...consentQuery(requestId ?? ""), enabled: requestId !== undefined });

  if (requestId === undefined) return <MissingRequest me={me} />;
  if (consent.isError) {
    if (consent.error instanceof ApiError && consent.error.status === 404) return <Expired />;
    return (
      <AuthLayout title={t("consent.loadFailed.title")} icon={<TriangleAlertIcon />}>
        <Alert tone="error">
          {consent.error instanceof ApiError
            ? consent.error.problem.detail || consent.error.problem.title
            : t("consent.failed")}
        </Alert>
        <Button variant="outline" className="h-11" onClick={() => void consent.refetch()}>
          {t("consent.loadFailed.retry")}
        </Button>
      </AuthLayout>
    );
  }
  if (!consent.data) {
    return (
      <AuthLayout title={t("consent.loadingTitle")} wide>
        <div aria-busy="true" aria-label={t("consent.loading")} className="flex flex-col gap-4">
          <div className="flex items-center gap-3">
            <Skeleton className="size-14 rounded-2xl" />
            <div className="flex flex-1 flex-col gap-2">
              <Skeleton className="h-5 w-40" />
              <Skeleton className="h-4 w-24" />
            </div>
          </div>
          <Skeleton className="h-48 w-full" />
          <Skeleton className="h-11 w-full" />
        </div>
      </AuthLayout>
    );
  }
  return <Consent key={consent.data.id} consent={consent.data} me={me} redirect={redirect} />;
}

type Busy = "approve" | "deny" | null;
type Done = { kind: "approved" | "denied" } | null;

function Consent({
  consent,
  me,
  redirect,
}: {
  consent: ConsentDto;
  me: MeDto;
  redirect: (url: string) => void;
}) {
  const { t } = useTranslation("ai");
  const roleLabel = useRoleLabel();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const ids = useId();
  const max = levelsOf(consent.grantable);
  const rows = areasOf(max);
  const [levels, setLevels] = useState<Levels>(max);
  const [busy, setBusy] = useState<Busy>(null);
  const [done, setDone] = useState<Done>(null);
  const [expired, setExpired] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const chosen = scopesOf(levels);
  const missing = beyondRole(levelsOf(consent.requested), max);
  const name = consent.client.name;
  const site = useQuery(metaQuery).data;
  const siteHost = site ? hostOf(site.site_url) : null;

  const decide = async (kind: "approve" | "deny") => {
    if (busy) return;
    setBusy(kind);
    setFailure(null);
    try {
      const to =
        kind === "approve"
          ? await approveConsent(consent.id, chosen)
          : await denyConsent(consent.id);
      const target = safeRedirect(to);
      if (target === null) throw new Error("refused redirect");
      // The request is used up. Its cached copy is never refetched here (a refetch would
      // answer 404 over "Connected"); a reload or Back reads it afresh and says expired.
      setDone({ kind: kind === "approve" ? "approved" : "denied" });
      redirect(target);
    } catch (error) {
      reportApiError(error);
      if (error instanceof ApiError && error.status === 404) {
        setExpired(true);
      } else if (error instanceof ApiError && error.status === 422) {
        setFailure(t("consent.noneChosen"));
      } else if (error instanceof ApiError && error.status === 403) {
        setFailure(error.problem.detail || t("consent.forbidden"));
      } else if (error instanceof ApiError && error.status !== 401) {
        setFailure(error.problem.detail || t("consent.failed"));
      } else {
        setFailure(t("consent.failed"));
      }
    } finally {
      setBusy(null);
    }
  };

  if (expired) return <Expired />;
  if (done) {
    const approved = done.kind === "approved";
    return (
      <AuthLayout
        title={approved ? t("consent.approved.title") : t("consent.denied.title")}
        description={
          approved
            ? t("consent.approved.body", { client: name })
            : t("consent.denied.body", { client: name })
        }
        icon={approved ? <CircleCheckIcon /> : <CircleSlashIcon />}
      >
        <span role="status" className="sr-only">
          {approved ? t("consent.approved.title") : t("consent.denied.title")}
        </span>
      </AuthLayout>
    );
  }

  const signOut = () => {
    const next = `/admin/connect?request=${encodeURIComponent(consent.id)}`;
    void logout({
      queryClient,
      navigate: (opts) => navigate({ ...opts, search: { next } }),
    });
  };

  const verifiedHost = consent.verified ? hostOf(consent.client.id) : null;
  const permissionsId = `${ids}-permissions`;
  const hintId = `${ids}-permissions-hint`;
  const noneId = `${ids}-none`;

  return (
    <AuthLayout
      wide
      title={t("consent.title", { client: name })}
      description={t("consent.description", {
        client: name,
        site: siteHost ?? t("consent.thisSite"),
      })}
      footer={
        <p className="flex flex-wrap items-center gap-x-1">
          <span>{t("consent.signedInAs", { login: me.login })}</span>
          <Button
            variant="link"
            className="h-11 px-0 text-sm"
            onClick={signOut}
            disabled={busy !== null}
          >
            {t("consent.notYou")}
          </Button>
        </p>
      }
    >
      <div
        data-slot="consent-client"
        className="flex items-start gap-4 rounded-xl border bg-muted/40 p-4"
      >
        <ClientLogo client={consent.client} size="lg" />
        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <span className="min-w-0 truncate text-base font-semibold text-foreground" title={name}>
              {name}
            </span>
            <ClientKind verified={consent.verified} />
          </div>
          <span className="flex min-w-0 items-baseline gap-1.5 text-sm text-muted-foreground">
            <span className="shrink-0">{t("consent.returnsTo")}</span>
            <span
              data-slot="consent-redirect"
              className="truncate font-mono text-[0.8125rem] text-foreground"
              title={consent.redirect_host}
            >
              {consent.redirect_host}
            </span>
          </span>
          {verifiedHost ? (
            <span className="text-xs text-muted-foreground">
              {t("consent.verifiedFrom", { host: verifiedHost })}
            </span>
          ) : null}
        </div>
      </div>
      {consent.verified ? null : (
        <p className="flex items-start gap-2 text-sm text-muted-foreground">
          <TriangleAlertIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-warning" />
          {t("consent.registeredWarning")}
        </p>
      )}

      {failure ? <Alert tone="error">{failure}</Alert> : null}

      {rows.length === 0 ? (
        <Alert tone="warning">{t("consent.nothingGrantable")}</Alert>
      ) : (
        <div className="flex min-w-0 flex-col gap-2">
          <div className="flex flex-wrap items-end justify-between gap-x-3 gap-y-1">
            <div className="flex min-w-0 flex-col gap-0.5">
              <span id={permissionsId} className="text-sm font-medium text-foreground">
                {t("consent.permissions")}
              </span>
              <span id={hintId} className="text-xs text-muted-foreground">
                {t("consent.permissionsHint")}
              </span>
            </div>
            <div className="-mr-2 flex shrink-0">
              <Button
                type="button"
                variant="ghost"
                className="text-muted-foreground"
                title={t("consent.readOnlyHint")}
                disabled={busy !== null}
                onClick={() => setLevels(readOnly(max))}
              >
                {t("consent.readOnly")}
              </Button>
              <Button
                type="button"
                variant="ghost"
                className="text-muted-foreground"
                title={t("consent.resetHint")}
                disabled={busy !== null}
                onClick={() => setLevels(max)}
              >
                {t("consent.reset")}
              </Button>
            </div>
          </div>
          <ul
            aria-labelledby={permissionsId}
            aria-describedby={hintId}
            data-slot="consent-permissions"
            className="flex flex-col divide-y rounded-lg border"
          >
            {rows.map((area) => (
              <PermissionRow
                key={area}
                name={`${ids}-${area}`}
                area={area}
                max={max[area] ?? "none"}
                value={levels[area] ?? "none"}
                disabled={busy !== null}
                onChange={(level) => setLevels((prev) => ({ ...prev, [area]: level }))}
              />
            ))}
          </ul>
          {missing.length > 0 ? (
            <p className="text-xs text-muted-foreground">
              {t("consent.beyondRole", {
                count: missing.length,
                areas: listOf(missing.map((a) => t(`areas.${a}.label`))),
              })}
            </p>
          ) : null}
        </div>
      )}

      <div className="flex items-start gap-3 rounded-lg border bg-card px-3 py-3 text-sm text-muted-foreground">
        <ShieldCheckIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-primary" />
        <div className="flex min-w-0 flex-col gap-1">
          <p className="text-foreground">
            {t("consent.actsAsYou", {
              client: name,
              name: me.display_name || me.login,
              role: roleLabel(me.role),
            })}
          </p>
          <p>{t("consent.disconnectAnyTime")}</p>
        </div>
      </div>

      <div className="flex flex-col gap-2">
        {chosen.length === 0 && rows.length > 0 ? (
          <p id={noneId} className="text-sm text-muted-foreground">
            {t("consent.noneChosen")}
          </p>
        ) : null}
        <div className="flex flex-col-reverse gap-3 sm:flex-row">
          <Button
            type="button"
            variant="outline"
            size="lg"
            className="h-11 w-full text-sm sm:w-auto sm:flex-1"
            disabled={busy !== null}
            aria-busy={busy === "deny" || undefined}
            onClick={() => void decide("deny")}
          >
            {busy === "deny" ? t("consent.denying") : t("consent.deny")}
          </Button>
          <Button
            type="button"
            size="lg"
            className="h-11 w-full text-sm sm:w-auto sm:flex-1"
            data-slot="consent-approve"
            disabled={busy !== null || chosen.length === 0}
            aria-describedby={chosen.length === 0 && rows.length > 0 ? noneId : undefined}
            aria-busy={busy === "approve" || undefined}
            onClick={() => void decide("approve")}
          >
            {busy === "approve" ? t("consent.approving") : t("consent.approve")}
          </Button>
        </div>
      </div>
    </AuthLayout>
  );
}

function PermissionRow({
  name,
  area,
  max,
  value,
  disabled,
  onChange,
}: {
  name: string;
  area: ScopeArea;
  max: Level;
  value: Level;
  disabled: boolean;
  onChange: (level: Level) => void;
}) {
  const { t } = useTranslation("ai");
  const labelId = `${name}-label`;
  const descId = `${name}-desc`;
  const levels: Level[] = max === "write" ? ["none", "read", "write"] : ["none", "read"];
  return (
    <li
      data-slot="consent-permission"
      data-area={area}
      data-level={value}
      className="flex items-center justify-between gap-3 px-3 py-2"
    >
      <div className="flex min-w-0 flex-col gap-0.5">
        <span id={labelId} className="text-sm font-medium text-foreground">
          {t(`areas.${area}.label`)}
        </span>
        <span id={descId} className="text-xs text-pretty text-muted-foreground">
          {value === "none" ? t("areas.off") : t(`areas.${area}.${value}`)}
        </span>
      </div>
      <div
        role="radiogroup"
        aria-labelledby={labelId}
        aria-describedby={descId}
        className="inline-flex shrink-0 gap-0.5 rounded-lg border bg-muted/50 p-0.5"
      >
        {levels.map((level) => (
          <label
            key={level}
            className={cn(
              "relative flex h-[calc(var(--control-h)-6px)] min-w-12 cursor-pointer items-center justify-center rounded-md px-2 text-sm text-muted-foreground transition-colors duration-(--dur-instant) select-none",
              "hover:text-foreground has-checked:bg-background has-checked:text-foreground has-checked:shadow-1",
              level === "write" && "has-checked:text-primary",
              "has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-ring",
              "has-disabled:cursor-not-allowed has-disabled:opacity-60",
            )}
          >
            <input
              type="radio"
              name={name}
              value={level}
              checked={value === level}
              disabled={disabled}
              onChange={() => onChange(level)}
              className="absolute inset-0 size-full cursor-pointer appearance-none opacity-0 disabled:cursor-not-allowed"
            />
            {t(`level.${level}`)}
          </label>
        ))}
      </div>
    </li>
  );
}

/** A request that is unknown, expired or already decided (or the switch is off). */
function Expired() {
  const { t } = useTranslation("ai");
  return (
    <AuthLayout
      title={t("consent.expired.title")}
      description={t("consent.expired.body")}
      icon={<TimerOffIcon />}
    >
      <Button asChild variant="outline" className="h-11">
        <Link to="/">{t("consent.expired.dashboard")}</Link>
      </Button>
    </AuthLayout>
  );
}

/** `/admin/connect` opened without a request: what the page is for. */
function MissingRequest({ me }: { me: MeDto }) {
  const { t } = useTranslation("ai");
  const settings = me.capabilities.includes("manage_settings");
  return (
    <AuthLayout
      title={t("consent.missing.title")}
      description={t("consent.missing.body")}
      icon={<ShieldCheckIcon />}
    >
      <Button asChild variant="outline" className="h-11">
        {settings ? (
          <Link to="/settings/$group" params={{ group: "ai-assistants" }}>
            {t("consent.missing.setup")}
          </Link>
        ) : (
          <Link to="/account">{t("consent.missing.setup")}</Link>
        )}
      </Button>
    </AuthLayout>
  );
}

function hostOf(url: string): string | null {
  try {
    return new URL(url).host;
  } catch {
    return null;
  }
}
