// Two-factor authentication (07 §2, H-A7). Off: "Set up" opens a two-step dialog: the
// QR code of `otpauth_url` and the key in groups of four, then the code and the password
// (`POST /auth/totp/confirm`, which ends every other session). On: "Turn off" asks for
// the password. The set-up answer holds the secret, so it is fetched with a plain call
// and lives only in the dialog body's state: closing the dialog unmounts the body and
// drops it, and the next opening starts a new enrolment (Review Focus 3).

import { useQueryClient } from "@tanstack/react-query";
import { cn } from "cn";
import { CheckIcon, CopyIcon, ShieldCheckIcon, ShieldIcon } from "lucide-react";
import { type FormEvent, useEffect, useEffectEvent, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { totpConfirm, totpDisable, totpSetup } from "@/api/auth";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import type { SchemaTotpSetupDto } from "@/api/schema";
import { FormAlert } from "@/auth/auth-layout";
import type { FormFailure } from "@/auth/errors";
import { PasswordField } from "@/auth/password-field";
import type { MeDto } from "@/auth/session";
import { CODE_DIGITS, CodeInput } from "@/components/kit/code-input";
import { QrCode } from "@/components/kit/qr-code";
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
import { Card, CardContent } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { DESTRUCTIVE } from "@/lib/destructive";
import { groupedSecret, isWrongPassword, writeFailure } from "./account-model";

/** Refreshes what a TOTP change touches: the user (`totp_enabled`) and the sessions. */
function useTotpChanged(userId: string) {
  const queryClient = useQueryClient();
  return () => {
    void queryClient.invalidateQueries({ queryKey: qk.me });
    void queryClient.invalidateQueries({ queryKey: qk.user(userId) });
    void queryClient.invalidateQueries({ queryKey: qk.sessions });
  };
}

export function TotpCard({ me }: { me: MeDto }) {
  const { t } = useTranslation("admin");
  const [setupOpen, setSetupOpen] = useState(false);
  const [disableOpen, setDisableOpen] = useState(false);
  const on = me.totp_enabled;
  const Icon = on ? ShieldCheckIcon : ShieldIcon;

  return (
    <Card>
      <CardContent className="flex flex-col gap-4">
        <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
          <div className="flex min-w-0 items-start gap-3">
            <span
              aria-hidden="true"
              className={cn(
                "flex size-10 shrink-0 items-center justify-center rounded-lg border",
                on
                  ? "border-success/40 bg-success/10 text-success"
                  : "bg-muted text-muted-foreground",
              )}
            >
              <Icon className="size-5" />
            </span>
            <div className="flex min-w-0 flex-col gap-1">
              <div className="flex items-center gap-2">
                <span className="text-sm font-medium text-foreground">
                  {t("account.totp.method")}
                </span>
                <Badge
                  data-slot="totp-status"
                  variant={on ? "outline" : "secondary"}
                  className={cn(on && "border-success/50 text-success")}
                >
                  {on ? t("account.totp.on") : t("account.totp.off")}
                </Badge>
              </div>
              <p className="max-w-prose text-sm text-muted-foreground">
                {on ? t("account.totp.onBody") : t("account.totp.offBody")}
              </p>
            </div>
          </div>
          {on ? (
            <Button variant="outline" className="self-start" onClick={() => setDisableOpen(true)}>
              {t("account.totp.turnOff")}
            </Button>
          ) : (
            <Button className="self-start" onClick={() => setSetupOpen(true)}>
              {t("account.totp.setUp")}
            </Button>
          )}
        </div>
        <p className="text-xs text-muted-foreground">{t("account.totp.lostHint")}</p>
      </CardContent>
      <TotpSetupDialog
        open={setupOpen}
        onOpenChange={setSetupOpen}
        userId={me.id}
        login={me.login}
      />
      <TotpDisableDialog
        open={disableOpen}
        onOpenChange={setDisableOpen}
        userId={me.id}
        login={me.login}
      />
    </Card>
  );
}

interface DialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  userId: string;
  login: string;
}

export function TotpSetupDialog({ open, onOpenChange, userId, login }: DialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="totp-setup" className="sm:max-w-xl">
        {/* Mounted only while open: its state (the secret) goes with it. */}
        <SetupBody close={() => onOpenChange(false)} userId={userId} login={login} />
      </DialogContent>
    </Dialog>
  );
}

type SetupState =
  | { kind: "loading" }
  | { kind: "failed"; alreadyOn: boolean }
  | { kind: "ready"; setup: SchemaTotpSetupDto };

function SetupBody({ close, userId, login }: { close: () => void; userId: string; login: string }) {
  const { t } = useTranslation("admin");
  const changed = useTotpChanged(userId);
  const [round, setRound] = useState(0);
  const [state, setState] = useState<SetupState>({ kind: "loading" });
  const [step, setStep] = useState<"scan" | "code">("scan");
  const [code, setCode] = useState("");
  const [password, setPassword] = useState("");
  const [codeError, setCodeError] = useState<string | null>(null);
  const [passwordError, setPasswordError] = useState<string | null>(null);
  const [expired, setExpired] = useState(false);
  const [failure, setFailure] = useState<FormFailure | null>(null);
  const [pending, setPending] = useState(false);
  const passwordInput = useRef<HTMLInputElement>(null);
  const codeInput = useRef<HTMLInputElement>(null);
  const ids = useId();
  const codeId = `${ids}-code`;
  const codeHintId = `${ids}-code-hint`;
  const codeErrorId = `${ids}-code-error`;
  const onAlreadyOn = useEffectEvent(changed);

  useEffect(() => {
    let live = true;
    totpSetup().then(
      (setup) => {
        if (live) setState({ kind: "ready", setup });
      },
      (error: unknown) => {
        if (!live) return;
        const alreadyOn = error instanceof ApiError && error.status === 422;
        if (alreadyOn) onAlreadyOn();
        else writeFailure(error);
        setState({ kind: "failed", alreadyOn });
      },
    );
    return () => {
      live = false;
    };
    // Each round is a new enrolment (Start again, Try again).
  }, [round]);

  const restart = () => {
    setState({ kind: "loading" });
    setStep("scan");
    setCode("");
    setCodeError(null);
    setExpired(false);
    setFailure(null);
    setRound((r) => r + 1);
  };

  const locked = failure?.kind === "rate-limited" && failure.seconds !== null;
  const confirm = async (e: FormEvent) => {
    e.preventDefault();
    if (state.kind !== "ready" || pending || locked) return;
    const badCode = code.length !== CODE_DIGITS ? t("account.totp.codeRequired") : null;
    const badPassword = password === "" ? t("account.passwordRequired") : null;
    setCodeError(badCode);
    setPasswordError(badPassword);
    if (badCode) {
      codeInput.current?.focus();
      return;
    }
    if (badPassword) {
      passwordInput.current?.focus();
      return;
    }
    setFailure(null);
    setPending(true);
    try {
      await totpConfirm({ enrollment: state.setup.enrollment, code, password });
      changed();
      toast.success(t("account.totp.enabled"), {
        description: t("account.totp.othersSignedOut"),
      });
      close();
    } catch (error) {
      if (isWrongPassword(error)) {
        setPasswordError(t("account.wrongPassword"));
        passwordInput.current?.focus();
      } else if (error instanceof ApiError && error.status === 422) {
        // Core names the part: `enrollment` (bad or ran out after 10 minutes) or `code`
        // (wrong or replayed); no field means TOTP is on already (another tab).
        const fields = error.fieldErrors();
        if (fields.enrollment !== undefined) {
          setExpired(true);
        } else if (fields.code !== undefined) {
          setCodeError(t("account.totp.codeWrong"));
          setCode("");
          codeInput.current?.focus();
        } else {
          changed();
          toast.info(t("account.totp.alreadyOn"));
          close();
        }
      } else {
        setFailure(writeFailure(error));
      }
    } finally {
      setPending(false);
    }
  };

  return (
    <form noValidate onSubmit={(e) => void confirm(e)} className="flex min-w-0 flex-col gap-5">
      <DialogHeader>
        <DialogTitle>{t("account.totp.setupTitle")}</DialogTitle>
        <DialogDescription>
          {step === "scan" ? t("account.totp.stepScan") : t("account.totp.stepCode")}
        </DialogDescription>
      </DialogHeader>

      {state.kind === "loading" ? (
        <div
          role="status"
          aria-label={t("account.totp.loading")}
          className="flex flex-col gap-5 sm:flex-row sm:items-center"
        >
          <Skeleton className="size-50 self-center rounded-md sm:self-auto" />
          <div className="flex flex-1 flex-col gap-2">
            <Skeleton className="h-4 w-48" />
            <Skeleton className="h-6 w-64 max-w-full" />
          </div>
        </div>
      ) : state.kind === "failed" ? (
        <div className="flex flex-col items-start gap-3">
          <p role="alert" className="text-sm text-foreground">
            {state.alreadyOn ? t("account.totp.alreadyOn") : t("account.totp.loadFailed")}
          </p>
          {state.alreadyOn ? null : (
            <Button type="button" variant="outline" onClick={restart}>
              {t("account.totp.retry")}
            </Button>
          )}
        </div>
      ) : step === "scan" ? (
        <ScanStep setup={state.setup} />
      ) : (
        <div className="flex flex-col gap-5">
          <input
            type="text"
            name="username"
            autoComplete="username"
            value={login}
            readOnly
            hidden
          />
          {expired ? (
            <div
              role="alert"
              className="flex flex-col items-start gap-2 rounded-lg border border-warning/50 bg-warning/8 px-3 py-2.5 text-sm text-foreground"
            >
              {t("account.totp.expired")}
              <Button type="button" size="sm" variant="outline" onClick={restart}>
                {t("account.totp.startAgain")}
              </Button>
            </div>
          ) : null}
          <FormAlert failure={failure} onExpired={() => setFailure(null)} />
          <div className="flex max-w-xs flex-col gap-2">
            <Label htmlFor={codeId}>{t("account.totp.code")}</Label>
            <CodeInput
              ref={codeInput}
              id={codeId}
              value={code}
              onChange={(v) => {
                setCode(v);
                setCodeError(null);
              }}
              onComplete={() => passwordInput.current?.focus()}
              describedBy={codeError ? `${codeHintId} ${codeErrorId}` : codeHintId}
              invalid={codeError !== null}
              disabled={pending}
            />
            <p id={codeHintId} className="text-xs text-muted-foreground">
              {t("account.totp.codeHint")}
            </p>
            {codeError ? (
              <p id={codeErrorId} className="text-sm text-destructive">
                {codeError}
              </p>
            ) : null}
          </div>
          <div className="max-w-xs">
            <PasswordField
              ref={passwordInput}
              id={`${ids}-password`}
              label={t("account.totp.password")}
              value={password}
              onValueChange={(v) => {
                setPassword(v);
                setPasswordError(null);
              }}
              autoComplete="current-password"
              error={passwordError}
              disabled={pending}
            />
          </div>
          <p className="text-sm text-muted-foreground">{t("account.totp.othersNote")}</p>
        </div>
      )}

      <DialogFooter>
        <Button type="button" variant="outline" onClick={close}>
          {t("account.totp.cancel")}
        </Button>
        {state.kind !== "ready" ? null : step === "scan" ? (
          <Button type="button" onClick={() => setStep("code")}>
            {t("account.totp.next")}
          </Button>
        ) : (
          <>
            <Button
              type="button"
              variant="ghost"
              onClick={() => {
                setStep("scan");
                setCodeError(null);
              }}
            >
              {t("account.totp.back")}
            </Button>
            <Button type="submit" disabled={pending || locked} aria-busy={pending}>
              {pending ? t("account.totp.turningOn") : t("account.totp.turnOn")}
            </Button>
          </>
        )}
      </DialogFooter>
    </form>
  );
}

function ScanStep({ setup }: { setup: SchemaTotpSetupDto }) {
  const { t } = useTranslation("admin");
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1_500);
    return () => window.clearTimeout(timer);
  }, [copied]);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(setup.secret_base32);
      setCopied(true);
      toast.success(t("account.totp.copied"), { id: "totp-copy" });
    } catch {
      toast.error(t("account.totp.copyFailed"), { id: "totp-copy" });
    }
  };
  return (
    <div className="flex flex-col gap-5 sm:flex-row sm:items-center">
      <QrCode
        text={setup.otpauth_url}
        label={t("account.totp.qrLabel")}
        className="self-center sm:self-auto"
      />
      <div className="flex min-w-0 flex-col gap-2">
        <span className="text-sm font-medium text-foreground">{t("account.totp.manual")}</span>
        <code
          data-slot="totp-secret"
          className="font-mono text-md tracking-wide break-words text-foreground select-all"
        >
          {groupedSecret(setup.secret_base32)}
        </code>
        <span className="text-xs text-muted-foreground">{t("account.totp.manualHint")}</span>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="self-start"
          onClick={() => void copy()}
        >
          {copied ? <CheckIcon data-icon="inline-start" /> : <CopyIcon data-icon="inline-start" />}
          {t("account.totp.copySecret")}
        </Button>
      </div>
    </div>
  );
}

export function TotpDisableDialog({ open, onOpenChange, userId, login }: DialogProps) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent data-slot="totp-disable">
        <DisableBody close={() => onOpenChange(false)} userId={userId} login={login} />
      </AlertDialogContent>
    </AlertDialog>
  );
}

function DisableBody({
  close,
  userId,
  login,
}: {
  close: () => void;
  userId: string;
  login: string;
}) {
  const { t } = useTranslation("admin");
  const changed = useTotpChanged(userId);
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [failure, setFailure] = useState<FormFailure | null>(null);
  const [pending, setPending] = useState(false);
  const locked = failure?.kind === "rate-limited" && failure.seconds !== null;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending || locked) return;
    if (password === "") {
      setError(t("account.passwordRequired"));
      input.current?.focus();
      return;
    }
    setFailure(null);
    setPending(true);
    try {
      await totpDisable(password);
      changed();
      toast.success(t("account.totp.disabled"));
      close();
    } catch (err) {
      if (isWrongPassword(err)) {
        setError(t("account.wrongPassword"));
        input.current?.focus();
      } else if (err instanceof ApiError && err.status === 422) {
        // Already off (another tab): say so and show it as it is.
        changed();
        toast.info(t("account.totp.alreadyOff"));
        close();
      } else {
        setFailure(writeFailure(err));
      }
    } finally {
      setPending(false);
    }
  };

  return (
    <form noValidate onSubmit={(e) => void submit(e)} className="flex min-w-0 flex-col gap-4">
      <AlertDialogHeader>
        <AlertDialogTitle>{t("account.totp.disableTitle")}</AlertDialogTitle>
        <AlertDialogDescription>{t("account.totp.disableBody")}</AlertDialogDescription>
      </AlertDialogHeader>
      <input type="text" name="username" autoComplete="username" value={login} readOnly hidden />
      <FormAlert failure={failure} onExpired={() => setFailure(null)} />
      <PasswordField
        ref={input}
        id={`${id}-password`}
        label={t("account.totp.password")}
        value={password}
        onValueChange={(v) => {
          setPassword(v);
          setError(null);
        }}
        autoComplete="current-password"
        error={error}
        disabled={pending}
        autoFocus
      />
      <AlertDialogFooter>
        <Button type="button" variant="outline" onClick={close}>
          {t("account.totp.cancel")}
        </Button>
        <Button
          type="submit"
          className={DESTRUCTIVE}
          disabled={pending || locked}
          aria-busy={pending}
        >
          {pending ? t("account.totp.turningOff") : t("account.totp.turnOff")}
        </Button>
      </AlertDialogFooter>
    </form>
  );
}
