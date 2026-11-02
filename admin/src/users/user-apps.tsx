// A user's connected apps (AI assistants, 12 §8) in the user sheet: an administrator
// sees them and can disconnect any (`GET /oauth/grants?user=`, `manage_users`).

import { SparklesIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { ConnectedApps } from "@/account/connected-apps-card";

/** `own`: the viewer's own row (no `?user=`). */
export function UserApps({
  userId,
  userName,
  own = false,
}: {
  userId: string;
  userName: string;
  own?: boolean;
}) {
  const { t } = useTranslation("ai");
  return (
    <div className="overflow-hidden rounded-lg border">
      <ConnectedApps
        userId={userId}
        own={own}
        userName={userName}
        rowClassName="px-4"
        empty={
          <p className="flex items-center gap-2 px-4 py-3 text-sm text-muted-foreground">
            <SparklesIcon aria-hidden="true" className="size-4 shrink-0" />
            {t("apps.emptyOther")}
          </p>
        }
      />
    </div>
  );
}
