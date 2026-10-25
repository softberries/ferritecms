// The frame of every dashboard card: a titled `<section>`, an optional scope badge and
// action, and its own loading / error state, so one failing card never takes the page
// down (07 §3; the rest of the dashboard keeps working).

import { cn } from "cn";
import { useId, type ReactNode } from "react";
import { ErrorState } from "@/components/kit/error-state";
import { Skeleton } from "@/components/ui/skeleton";

export interface PanelProps {
  title: string;
  description?: ReactNode;
  /** A small scope note beside the title ("This node only"). */
  scope?: ReactNode;
  /** A link or control at the header's end. */
  action?: ReactNode;
  footer?: ReactNode;
  /** Still loading: `aria-busy`, so assistive tech waits for the content. */
  busy?: boolean;
  className?: string;
  children: ReactNode;
}

export function Panel({
  title,
  description,
  scope,
  action,
  footer,
  busy = false,
  className,
  children,
}: PanelProps) {
  const id = useId();
  return (
    <section
      aria-labelledby={id}
      aria-busy={busy || undefined}
      data-slot="dashboard-panel"
      className={cn(
        "flex min-w-0 flex-col rounded-xl border bg-card text-card-foreground shadow-1",
        className,
      )}
    >
      <header className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2 px-5 pt-4">
        <div className="flex min-w-0 flex-col gap-0.5">
          <div className="flex flex-wrap items-center gap-2">
            <h2 id={id} className="text-md font-semibold text-foreground">
              {title}
            </h2>
            {scope}
          </div>
          {description ? <p className="text-sm text-muted-foreground">{description}</p> : null}
        </div>
        {action ? <div className="flex shrink-0 items-center gap-2">{action}</div> : null}
      </header>
      <div className="flex min-w-0 flex-1 flex-col px-5 pt-4 pb-5">{children}</div>
      {footer ? <footer className="border-t px-5 py-3 text-sm">{footer}</footer> : null}
    </section>
  );
}

/** A quiet pill beside a panel title naming what the numbers cover. */
export function ScopeBadge({ children }: { children: ReactNode }) {
  return (
    <span className="inline-flex h-5 items-center rounded-full border px-2 text-xs font-medium text-muted-foreground">
      {children}
    </span>
  );
}

/** A panel's failed load, sized for a card (the page around it keeps working). */
export function PanelError({ error, retry }: { error: unknown; retry: () => void }) {
  return <ErrorState error={error} retry={retry} className="flex-1 border-dashed py-8" />;
}

/** Skeleton rows shaped like a list panel. */
export function ListSkeleton({ rows = 4 }: { rows?: number }) {
  return (
    <div aria-hidden="true" className="flex flex-col gap-3" data-slot="list-skeleton">
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="flex items-center gap-3">
          <Skeleton className="size-2 rounded-full" />
          <Skeleton className="h-4 flex-1" />
          <Skeleton className="h-4 w-16" />
        </div>
      ))}
    </div>
  );
}
