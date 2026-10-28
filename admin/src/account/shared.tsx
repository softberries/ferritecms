// The frame of each part of the account screen: a heading and description beside the
// card on wide screens, above it on phones.

import type { ReactNode } from "react";

export interface AccountSectionProps {
  id: string;
  title: string;
  description: string;
  children: ReactNode;
}

/** One part of the account screen: an ARIA region named by its heading. */
export function AccountSection({ id, title, description, children }: AccountSectionProps) {
  const titleId = `${id}-title`;
  return (
    <section
      aria-labelledby={titleId}
      data-slot={`account-${id}`}
      className="grid gap-4 border-t pt-8 lg:grid-cols-[16rem_minmax(0,1fr)] lg:gap-10"
    >
      <div className="flex flex-col gap-1">
        <h2 id={titleId} className="text-lg text-foreground">
          {title}
        </h2>
        <p className="max-w-prose text-sm text-muted-foreground">{description}</p>
      </div>
      <div className="min-w-0">{children}</div>
    </section>
  );
}
