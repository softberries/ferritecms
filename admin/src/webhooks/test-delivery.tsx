// Send test (design 18 §10.1): queues a `webhook.ping`, then polls that delivery every
// second for up to 20 seconds and says how it went, inline (`aria-live="polite"`).

import { useQueryClient } from "@tanstack/react-query";
import { SendIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { getDelivery, testWebhook } from "@/api/webhooks";
import { Button } from "@/components/ui/button";
import { problemText } from "@/structure/taxonomies/terms-model";

/** How often, and how long, the test's delivery is polled. */
export const TEST_POLL_MS = 1_000;
export const TEST_POLL_LIMIT_MS = 20_000;

type Outcome =
  | { kind: "idle" }
  | { kind: "sending" }
  | { kind: "delivered"; code: number | null; ms: number }
  | { kind: "failed"; error: string }
  | { kind: "waiting" }
  | { kind: "error"; message: string };

export function TestDelivery({
  id,
  variant = "outline",
}: {
  id: string;
  variant?: "outline" | "default";
}) {
  const { t } = useTranslation("webhooks");
  const queryClient = useQueryClient();
  const [outcome, setOutcome] = useState<Outcome>({ kind: "idle" });
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const run = async () => {
    setOutcome({ kind: "sending" });
    let delivery: string;
    try {
      delivery = await testWebhook(id);
    } catch (error) {
      reportApiError(error);
      setOutcome({ kind: "error", message: problemText(error) });
      return;
    }
    const started = Date.now();
    while (alive.current && Date.now() - started < TEST_POLL_LIMIT_MS) {
      await new Promise((r) => setTimeout(r, TEST_POLL_MS));
      if (!alive.current) return;
      try {
        const d = await getDelivery(id, delivery);
        if (d.delivery.status === "pending") continue;
        const last = d.attempts[d.attempts.length - 1];
        setOutcome(
          d.delivery.status === "delivered"
            ? { kind: "delivered", code: last?.status_code ?? null, ms: last?.duration_ms ?? 0 }
            : { kind: "failed", error: d.delivery.last_error ?? d.delivery.status },
        );
        void queryClient.invalidateQueries({ queryKey: qk.webhooks.item(id) });
        void queryClient.invalidateQueries({ queryKey: qk.webhooks.list });
        return;
      } catch {
        // A read that failed: try again on the next tick.
      }
    }
    if (alive.current) setOutcome({ kind: "waiting" });
  };

  const text =
    outcome.kind === "sending"
      ? t("test.sending")
      : outcome.kind === "delivered"
        ? t("test.delivered", { code: outcome.code ?? "–", ms: outcome.ms })
        : outcome.kind === "failed"
          ? t("test.failed", { error: outcome.error })
          : outcome.kind === "waiting"
            ? t("test.waiting")
            : outcome.kind === "error"
              ? outcome.message
              : "";

  return (
    <div
      data-slot="test-delivery"
      className="flex min-w-0 flex-col gap-2 sm:flex-row sm:items-center"
    >
      <Button
        type="button"
        variant={variant}
        disabled={outcome.kind === "sending"}
        aria-busy={outcome.kind === "sending" || undefined}
        onClick={() => void run()}
      >
        <SendIcon data-icon="inline-start" />
        {t("test.action")}
      </Button>
      <p
        aria-live="polite"
        data-slot="test-result"
        data-outcome={outcome.kind}
        className={
          outcome.kind === "delivered"
            ? "text-sm text-success"
            : outcome.kind === "failed" || outcome.kind === "error"
              ? "text-sm text-destructive"
              : "text-sm text-muted-foreground"
        }
      >
        {text}
      </p>
    </div>
  );
}
