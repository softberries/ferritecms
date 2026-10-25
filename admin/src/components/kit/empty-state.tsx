// What a screen shows when there is nothing yet: what it is for and the way in.

import type { ComponentType, ReactNode } from "react";
import { cn } from "cn";

export interface EmptyStateProps {
  icon: ComponentType<{ className?: string }>;
  title: ReactNode;
  body: ReactNode;
  action?: ReactNode;
  className?: string;
}

export function EmptyState({ icon: Icon, title, body, action, className }: EmptyStateProps) {
  return (
    <div
      data-slot="empty-state"
      className={cn(
        "flex flex-col items-center justify-center gap-4 rounded-xl border border-dashed px-6 py-12 text-center",
        className,
      )}
    >
      <span className="flex size-11 items-center justify-center rounded-lg border bg-card text-muted-foreground shadow-1">
        <Icon className="size-5" />
      </span>
      <div className="flex max-w-sm flex-col gap-1">
        <h2 className="text-md text-foreground">{title}</h2>
        <p className="text-muted-foreground">{body}</p>
      </div>
      {action ? <div className="flex flex-wrap justify-center gap-2">{action}</div> : null}
    </div>
  );
}
