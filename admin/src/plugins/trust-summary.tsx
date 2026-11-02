// Who made a plugin and whose data it gets (M6-2, design 14 §10): the Publisher block at
// the top of a permission report, and the publisher line of the plugin list and page.
// Verified (a registry publisher's key), signed by a key that is new to the site, signed
// by a key the site trusts, or unsigned; then what the install does to the data of the
// plugin's name. Every string from the manifest, the registry or a key is text. The state
// is always said in words, never by colour alone.

import { cn } from "cn";
import {
  BadgeCheckIcon,
  KeyRoundIcon,
  ShieldAlertIcon,
  ShieldOffIcon,
  type LucideIcon,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type { PluginDto, TrustDto } from "@/api/plugins";
import { publisherText } from "./plugin-model";

type Tone = "verified" | "warning" | "danger" | "neutral";

const TONE: Record<Tone, string> = {
  verified: "border-success/40 bg-success/6",
  warning: "border-warning/50 bg-warning/8",
  danger: "border-destructive/40 bg-destructive/6",
  neutral: "bg-card",
};

const ICON_TONE: Record<Tone, string> = {
  verified: "text-success",
  warning: "text-warning",
  danger: "text-destructive",
  neutral: "text-muted-foreground",
};

/** The Publisher block of an upload's or a registry version's permission report. */
export function TrustSummary({ trust }: { trust: TrustDto }) {
  const { t } = useTranslation("admin");
  const signer = trust.signer;
  let tone: Tone;
  let Icon: LucideIcon;
  let title: string;
  let body: string | null;
  let state: string;
  if (trust.refusal) {
    tone = "danger";
    Icon = ShieldOffIcon;
    title = t("plugins.trust.refused");
    body = trust.refusal.message;
    state = "refused";
  } else if (!signer) {
    tone = "danger";
    Icon = ShieldAlertIcon;
    title = t("plugins.trust.unsigned");
    body = t("plugins.trust.unsignedBody");
    state = "unsigned";
  } else if (signer.source === "registry") {
    tone = "verified";
    Icon = BadgeCheckIcon;
    title = t("plugins.trust.verified");
    body = signer.publisher?.name ?? signer.claimed;
    state = "verified";
  } else if (signer.source === "new") {
    tone = "warning";
    Icon = KeyRoundIcon;
    title = t("plugins.trust.newKey");
    body = t("plugins.trust.newKeyBody");
    state = "new_key";
  } else {
    tone = "neutral";
    Icon = KeyRoundIcon;
    title = t("plugins.trust.signed");
    body = null;
    state = "signed";
  }
  const owner = trust.owner;
  const ownerLine =
    owner?.change === "claim"
      ? t("plugins.trust.claim", { owner: owner.next_label ?? owner.next ?? "" })
      : owner?.change === "bind"
        ? t("plugins.trust.bind", { owner: owner.next_label ?? owner.next ?? "" })
        : owner?.change === "unsigned_update"
          ? t("plugins.trust.unsignedReadsData")
          : null;
  const policyShaped = trust.policy !== "any" || trust.refusal?.field === "signature";
  return (
    <div
      role="group"
      aria-label={t("plugins.trust.label")}
      data-slot="trust-summary"
      data-state={state}
      className={cn("flex min-w-0 items-start gap-3 rounded-lg border px-3 py-2.5", TONE[tone])}
    >
      <Icon aria-hidden="true" className={cn("mt-0.5 size-4 shrink-0", ICON_TONE[tone])} />
      <div className="flex min-w-0 flex-1 flex-col gap-1 text-sm">
        <p data-slot="trust-title" className="font-medium text-foreground">
          {title}
        </p>
        {body ? <p className="break-words text-foreground">{body}</p> : null}
        {signer ? (
          <p className="text-muted-foreground">
            {t("plugins.trust.key")}{" "}
            <span data-slot="trust-key" className="font-mono text-xs text-foreground">
              {signer.key_id}
            </span>
          </p>
        ) : null}
        {signer && signer.source !== "registry" ? (
          <p data-slot="trust-claim" className="break-words text-muted-foreground">
            {t("plugins.trust.claims", { publisher: signer.claimed })}
          </p>
        ) : null}
        {ownerLine ? (
          <p
            data-slot="trust-owner"
            className={cn(
              "break-words",
              owner?.change === "bind" ? "text-muted-foreground" : "text-foreground",
            )}
          >
            {ownerLine}
          </p>
        ) : null}
        {policyShaped ? (
          <p data-slot="trust-policy" className="text-xs text-muted-foreground">
            {t("plugins.trust.policy", { policy: trust.policy })}
          </p>
        ) : null}
      </div>
    </div>
  );
}

/** The publisher cell of the plugin list (text, with an icon for the verified state). */
export function PublisherLine({ plugin }: { plugin: PluginDto }) {
  const { t } = useTranslation("admin");
  const text = publisherText(t, plugin);
  const state = plugin.trust.state;
  return (
    <span
      data-slot="plugin-publisher"
      data-trust={state}
      title={text}
      className={cn(
        "inline-flex min-w-0 items-center gap-1 truncate text-xs",
        state === "revoked" ? "text-destructive" : "text-muted-foreground",
      )}
    >
      {state === "verified" ? (
        <BadgeCheckIcon aria-hidden="true" className="size-3.5 shrink-0 text-success" />
      ) : null}
      <span className="truncate">{text}</span>
    </span>
  );
}
