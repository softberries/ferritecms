// A failed load: the problem's `detail` (07 §1), a retry, never a stack trace.

import { CircleAlertIcon, RotateCwIcon } from "lucide-react";
import { cn } from "cn";
import { useTranslation } from "react-i18next";
import { ApiError } from "@/api/problem";
import { Button } from "@/components/ui/button";

export interface ErrorStateProps {
  error: unknown;
  retry: () => void;
  className?: string;
}

/** The text a user may see: an API problem's detail (or title); anything else is generic. */
function errorMessage(error: unknown): string | null {
  if (error instanceof ApiError) return error.problem.detail || error.problem.title || null;
  return null;
}

export function ErrorState({ error, retry, className }: ErrorStateProps) {
  const { t } = useTranslation();
  const detail = errorMessage(error) ?? t("kit.error.unknown");
  return (
    <div
      role="alert"
      data-slot="error-state"
      className={cn(
        "flex flex-col items-center justify-center gap-4 rounded-xl border px-6 py-12 text-center",
        className,
      )}
    >
      <span className="flex size-11 items-center justify-center rounded-lg border bg-card text-destructive shadow-1">
        <CircleAlertIcon className="size-5" />
      </span>
      <div className="flex max-w-md flex-col gap-1">
        <h2 className="text-md text-foreground">{t("kit.error.title")}</h2>
        <p className="text-muted-foreground">{detail}</p>
      </div>
      <Button variant="outline" onClick={retry}>
        <RotateCwIcon data-icon="inline-start" />
        {t("kit.error.retry")}
      </Button>
    </div>
  );
}
