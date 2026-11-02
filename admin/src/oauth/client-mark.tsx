// How an OAuth client shows (the consent screen, connected apps): its logo when the
// site verified it through its metadata document and it declared an `https` one (loaded
// without a referrer; a failed load falls back), else a monogram (a self-registered
// client's logo is unchecked, so it never shows); and whether the client is verified
// or registered itself. The name is the client's own text and renders as text.

import { cn } from "cn";
import { BadgeCheckIcon, CircleDashedIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { OAuthClientDto } from "@/api/oauth";
import { monogram } from "./model";

export function ClientLogo({
  client,
  size = "md",
}: {
  client: Pick<OAuthClientDto, "name" | "logo_uri" | "kind">;
  size?: "md" | "lg";
}) {
  const [failed, setFailed] = useState(false);
  const verified = client.kind === "metadata";
  const logo =
    verified && client.logo_uri?.startsWith("https://") && !failed ? client.logo_uri : null;
  const box = size === "lg" ? "size-14 rounded-2xl text-xl" : "size-10 rounded-xl text-base";
  return (
    <span
      aria-hidden="true"
      data-slot="client-logo"
      className={cn(
        "flex shrink-0 items-center justify-center overflow-hidden border bg-muted font-semibold text-foreground shadow-1",
        box,
      )}
    >
      {logo ? (
        <img
          src={logo}
          alt=""
          referrerPolicy="no-referrer"
          decoding="async"
          className="size-full bg-card object-contain p-1.5"
          onError={() => setFailed(true)}
        />
      ) : (
        monogram(client.name)
      )}
    </span>
  );
}

/** Verified by its metadata document, or self-registered (with what that means). */
export function ClientKind({ verified, className }: { verified: boolean; className?: string }) {
  const { t } = useTranslation("ai");
  const Icon = verified ? BadgeCheckIcon : CircleDashedIcon;
  return (
    <span
      data-slot="client-kind"
      data-verified={verified}
      title={verified ? t("client.verifiedHint") : t("client.registeredHint")}
      className={cn(
        "inline-flex h-6 items-center gap-1 rounded-full border px-2 text-xs font-medium whitespace-nowrap",
        verified ? "border-success/40 text-success" : "text-muted-foreground",
        className,
      )}
    >
      <Icon aria-hidden="true" className="size-3.5" />
      {verified ? t("client.verified") : t("client.registered")}
    </span>
  );
}
