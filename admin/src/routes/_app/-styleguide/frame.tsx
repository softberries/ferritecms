// Layout pieces of the styleguide: a numbered section, a titled block, a specimen
// panel and the state matrix (default / hover / focus / disabled / loading / error).

import { cn } from "cn";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { SectionId } from "./sections";
import { SECTIONS } from "./sections";

export function Section({ id, children }: { id: SectionId; children: ReactNode }) {
  const { t } = useTranslation("styleguide");
  const index = SECTIONS.indexOf(id) + 1;
  return (
    <section
      id={id}
      aria-labelledby={`${id}-title`}
      data-section={id}
      className="flex scroll-mt-24 flex-col gap-8 border-t pt-10 first:border-t-0 first:pt-0"
    >
      <header className="flex flex-col gap-2">
        <span className="font-mono text-xs text-primary tabular-nums">
          {String(index).padStart(2, "0")}
        </span>
        <h2 id={`${id}-title`} className="text-2xl">
          {t(`sections.${id}.title`)}
        </h2>
        <p className="max-w-2xl text-md text-muted-foreground">{t(`sections.${id}.lead`)}</p>
      </header>
      {children}
    </section>
  );
}

export function Block({
  title,
  description,
  children,
  className,
}: {
  title: ReactNode;
  description?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("flex min-w-0 flex-col gap-3", className)}>
      <div className="flex flex-col gap-0.5">
        <h3 className="text-md">{title}</h3>
        {description ? (
          <p className="max-w-2xl text-sm text-muted-foreground">{description}</p>
        ) : null}
      </div>
      {children}
    </div>
  );
}

/** A bordered panel holding live components. */
export function Specimen({
  children,
  className,
  padded = true,
}: {
  children: ReactNode;
  className?: string;
  padded?: boolean;
}) {
  return (
    <div
      className={cn(
        "min-w-0 rounded-xl border bg-card text-card-foreground",
        padded && "p-5",
        className,
      )}
    >
      {children}
    </div>
  );
}

export type StateName = "default" | "hover" | "focus" | "disabled" | "loading" | "error";

/**
 * Rows of components across their states. `render(state)` returns the component in that
 * state, or `null` where the state does not apply.
 */
export function StateMatrix({
  states,
  rows,
}: {
  states: StateName[];
  rows: { label: ReactNode; render: (state: StateName) => ReactNode }[];
}) {
  const { t } = useTranslation("styleguide");
  return (
    <Specimen padded={false} className="overflow-x-auto">
      <table className="w-full border-separate border-spacing-0 text-sm">
        <thead>
          <tr>
            <th scope="col" className="sticky left-0 z-10 bg-card px-4 py-3 text-left">
              <span className="sr-only">{t("states.component")}</span>
            </th>
            {states.map((s) => (
              <th
                key={s}
                scope="col"
                className="px-4 py-3 text-left text-xs font-medium tracking-wide text-muted-foreground uppercase"
              >
                {t(`states.${s}`)}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, i) => (
            <tr key={i}>
              <th
                scope="row"
                className="sticky left-0 z-10 border-t bg-card px-4 py-4 text-left font-mono text-xs font-normal whitespace-nowrap text-muted-foreground"
              >
                {row.label}
              </th>
              {states.map((s) => (
                <td key={s} className="border-t px-4 py-4 align-middle">
                  {row.render(s) ?? (
                    <span aria-hidden="true" className="text-muted-foreground">
                      —
                    </span>
                  )}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </Specimen>
  );
}

/** Classes that put a component into a state statically (`.force-hover`, globals.css). */
export function stateClass(state: StateName): string | undefined {
  if (state === "hover") return "force-hover";
  if (state === "focus") return "force-focus";
  return undefined;
}
