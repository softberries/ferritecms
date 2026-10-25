import { createFileRoute } from "@tanstack/react-router";
import { LogOutIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useLogout, useMe } from "@/auth/session";
import { Button } from "@/components/ui/button";

// Placeholder dashboard behind the login guard (pre-flight F19); Task 12 replaces it.
export const Route = createFileRoute("/_app/")({
  component: IndexPage,
});

function IndexPage() {
  const { t } = useTranslation();
  const { t: ta } = useTranslation("auth");
  const me = useMe();
  const logout = useLogout();
  return (
    <main id="main" className="mx-auto flex min-h-svh max-w-xl flex-col justify-center gap-2 p-8">
      <p className="font-mono text-xs tracking-wider text-primary uppercase">{t("app.tagline")}</p>
      <h1 className="text-3xl">{t("app.name")}</h1>
      <p className="text-muted-foreground">{t("placeholder.body")}</p>
      {me ? (
        <div className="mt-6 flex flex-wrap items-center justify-between gap-3 rounded-xl border bg-card px-4 py-3">
          <p className="text-sm">{ta("app.signedInAs", { name: me.display_name })}</p>
          <Button
            variant="outline"
            onClick={() => {
              void logout();
            }}
          >
            <LogOutIcon data-icon="inline-start" />
            {ta("app.signOut")}
          </Button>
        </div>
      ) : null}
    </main>
  );
}
