// The header every screen starts with: title, description, actions, tabs (11 §8).

import type { ReactNode } from "react";
import { cn } from "cn";

export interface PageHeaderProps {
  title: ReactNode;
  description?: ReactNode;
  /** One primary action at most; secondary actions before it. */
  actions?: ReactNode;
  /** Usually a `TabsList` (status tabs on lists). */
  tabs?: ReactNode;
  className?: string;
}

export function PageHeader({ title, description, actions, tabs, className }: PageHeaderProps) {
  return (
    <header data-slot="page-header" className={cn("flex flex-col gap-4", className)}>
      <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
        <div className="flex min-w-0 flex-col gap-1">
          <h1 className="text-2xl text-foreground">{title}</h1>
          {description ? (
            <p className="max-w-prose text-md text-muted-foreground">{description}</p>
          ) : null}
        </div>
        {actions ? (
          <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div>
        ) : null}
      </div>
      {tabs ? <div className="-mb-px overflow-x-auto">{tabs}</div> : null}
    </header>
  );
}
