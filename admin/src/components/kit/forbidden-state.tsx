// A screen the server says this account may not see (the SPA hides, it never decides).

import { LockKeyholeIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { EmptyState } from "./empty-state";

export function ForbiddenState({ className }: { className?: string }) {
  const { t } = useTranslation();
  return (
    <EmptyState
      icon={LockKeyholeIcon}
      title={t("kit.forbidden.title")}
      body={t("kit.forbidden.body")}
      {...(className ? { className } : {})}
    />
  );
}
