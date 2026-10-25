import { Outlet, createRootRouteWithContext } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import type { RouterContext } from "@/lib/router-context";

export const Route = createRootRouteWithContext<RouterContext>()({
  component: RootLayout,
});

function RootLayout() {
  const { t } = useTranslation();
  return (
    <>
      <title>{t("app.name")}</title>
      <div className="min-h-svh bg-background text-foreground">
        <Outlet />
      </div>
    </>
  );
}
