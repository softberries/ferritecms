import { createFileRoute } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";

// Placeholder: Task 9 moves this route to `routes/_app/index.tsx` (pre-flight F19).
export const Route = createFileRoute("/")({
  component: IndexPage,
});

function IndexPage() {
  const { t } = useTranslation();
  return (
    <main className="mx-auto flex min-h-svh max-w-xl flex-col justify-center gap-2 p-8">
      <p className="font-mono text-xs tracking-wider text-primary uppercase">{t("app.tagline")}</p>
      <h1 className="text-3xl">{t("app.name")}</h1>
      <p className="text-muted-foreground">{t("placeholder.body")}</p>
    </main>
  );
}
