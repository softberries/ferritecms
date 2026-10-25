// The not-found screen (pre-flight F20: the router's `defaultNotFoundComponent`, not a
// `/not-found` route). Under `_app` it renders inside the shell, behind the guard.

import { Link } from "@tanstack/react-router";
import { CompassIcon, LayoutDashboardIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { EmptyState } from "@/components/kit/empty-state";
import { Button } from "@/components/ui/button";

export function NotFound() {
  const { t } = useTranslation("shell");
  return (
    <div
      data-slot="not-found"
      className="mx-auto flex w-full max-w-xl flex-1 flex-col justify-center py-8"
    >
      <EmptyState
        icon={CompassIcon}
        title={<span className="text-lg">{t("notFound.title")}</span>}
        body={t("notFound.body")}
        action={
          <Button asChild>
            <Link to="/">
              <LayoutDashboardIcon data-icon="inline-start" />
              {t("notFound.action")}
            </Link>
          </Button>
        }
      />
    </div>
  );
}
