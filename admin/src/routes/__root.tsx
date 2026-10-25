import { Outlet, createRootRouteWithContext, useMatches } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import type { RouterContext } from "@/lib/router-context";

export const Route = createRootRouteWithContext<RouterContext>()({
  component: RootLayout,
});

function RootLayout() {
  const { t } = useTranslation();
  const { t: ts } = useTranslation("shell");
  // One <title> for the whole app: the deepest route's crumb, then the product name.
  const crumb = useMatches({
    select: (matches) => {
      for (let i = matches.length - 1; i >= 0; i--) {
        const match = matches[i];
        const label = match?.staticData.crumb?.(ts, match.loaderData);
        if (label) return label;
      }
      return null;
    },
  });
  const name = t("app.name");
  return (
    <>
      <title>{crumb ? `${crumb} · ${name}` : name}</title>
      <div className="min-h-svh bg-background text-foreground">
        <Outlet />
      </div>
    </>
  );
}
