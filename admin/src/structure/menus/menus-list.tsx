// The menus (07 §3 Menus; `manage_menus`): the active theme's locations first, each with
// the menu shown there (a menu named like the location) or "Create menu", then every
// other menu. New menu takes a label and a name derived from it until typed (core's rule:
// `slug::validate`, lower case, unchanged by it); `PUT /menus/{name}` would replace a
// menu that exists, so the name is checked against the server first.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { ListTreeIcon, MapPinIcon, PencilIcon, PlusIcon } from "lucide-react";
import { useState, type FormEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { type MenuDto, listMenus, menusQuery, putMenu } from "@/api/menus";
import { useSiteLocales } from "@/lib/locales";
import { LocationLocales } from "./menu-locales";
import { ApiError } from "@/api/problem";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { problemText } from "../taxonomies/terms-model";
import { without } from "../shared";
import { FormField } from "@/components/kit/form-field";
import { MAX_MENU_LABEL, MAX_MENU_NAME, menuNameProblem, nameFromMenuLabel } from "./menu-model";

/** Thrown by `createMenu` when the name is taken. */
class MenuExists extends Error {}

/**
 * Creates an empty menu, refusing a name the server lists (a `PUT` would replace that
 * menu). The gap between the check and the write is accepted, as for the stale guard.
 */
async function createMenu(name: string, label: string): Promise<MenuDto> {
  const { items } = await listMenus();
  if (items.some((m) => m.name === name)) throw new MenuExists(name);
  return putMenu(name, { label, items: [] });
}

export interface MenusListProps {
  /** The New menu dialog is open (`?new=true`). */
  creating: boolean;
  setCreating: (open: boolean) => void;
}

export function MenusList({ creating, setCreating }: MenusListProps) {
  const { t } = useTranslation("structure");
  const qc = useQueryClient();
  const navigate = useNavigate();
  const list = useQuery({ ...menusQuery, refetchOnMount: "always" });
  const [pending, setPending] = useState<string | null>(null);
  const menus = list.data?.items ?? [];
  const locations = list.data?.locations ?? [];
  const located = new Set(locations.map((l) => l.name));
  const locales = useSiteLocales();
  // A location's menu in another locale is listed under its location (M6-1).
  const others = menus
    .filter((m) => !located.has(m.name))
    .filter((m) => !(m.translation_of && located.has(m.translation_of)))
    .sort((a, b) => (a.label || a.name).localeCompare(b.label || b.name));
  const labelOf = (name: string) => menus.find((m) => m.name === name)?.label ?? "";

  const open = (name: string) => void navigate({ to: "/structure/menus/$name", params: { name } });
  /** "Create menu" on a location: the menu named like it, labelled like it. */
  const createFor = async (name: string, label: string) => {
    setPending(name);
    try {
      await createMenu(name, label);
      void qc.invalidateQueries({ queryKey: qk.menus.list });
      toast.success(t("menus.create.created", { label }));
    } catch (e) {
      reportApiError(e);
      // Someone made it meanwhile: open theirs.
      if (!(e instanceof MenuExists)) {
        toast.error(t("menus.create.failed"), { description: problemText(e) });
        return;
      }
    } finally {
      setPending(null);
    }
    open(name);
  };

  const newButton = (variant: "default" | "outline") => (
    <Button variant={variant} onClick={() => setCreating(true)}>
      <PlusIcon data-icon="inline-start" />
      {t("menus.new")}
    </Button>
  );

  let body: ReactNode;
  if (list.isError && !list.data) {
    body = <ErrorState error={list.error} retry={() => void list.refetch()} />;
  } else if (list.isPending) {
    body = <ListSkeleton />;
  } else if (menus.length === 0 && locations.length === 0) {
    body = (
      <EmptyState
        icon={ListTreeIcon}
        title={t("menus.empty.title")}
        body={t("menus.empty.body")}
        action={newButton("outline")}
      />
    );
  } else {
    body = (
      <>
        <Section
          slot="menu-locations"
          title={t("menus.locations.title")}
          hint={locations.length > 0 ? t("menus.locations.hint") : t("menus.locations.none")}
        >
          {locations.length > 0 ? (
            <ul className="flex flex-col gap-2">
              {locations.map((loc) => (
                <Row
                  key={loc.name}
                  slot="menu-location"
                  name={loc.name}
                  icon={MapPinIcon}
                  title={loc.label || loc.name}
                  detail={
                    <>
                      {loc.menu ? (
                        t("menus.locations.shows", {
                          label: labelOf(loc.name) || t("menus.unlabelled"),
                        })
                      ) : (
                        <Badge variant="outline">{t("menus.locations.empty")}</Badge>
                      )}
                      <LocationLocales name={loc.name} menus={menus} locales={locales} />
                    </>
                  }
                  action={
                    loc.menu ? (
                      <EditLink
                        name={loc.name}
                        label={labelOf(loc.name) || loc.label || loc.name}
                      />
                    ) : (
                      <Button
                        variant="outline"
                        disabled={pending !== null}
                        aria-label={t("menus.locations.createFor", {
                          label: loc.label || loc.name,
                        })}
                        onClick={() => void createFor(loc.name, loc.label || loc.name)}
                      >
                        <PlusIcon data-icon="inline-start" />
                        {t("menus.locations.create")}
                      </Button>
                    )
                  }
                />
              ))}
            </ul>
          ) : null}
        </Section>
        <Section slot="menu-others" title={t("menus.others.title")} hint={t("menus.others.hint")}>
          {others.length > 0 ? (
            <ul className="flex flex-col gap-2">
              {others.map((m) => (
                <Row
                  key={m.name}
                  slot="menu-row"
                  name={m.name}
                  icon={ListTreeIcon}
                  title={m.label || t("menus.unlabelled")}
                  action={<EditLink name={m.name} label={m.label || m.name} />}
                />
              ))}
            </ul>
          ) : (
            <p className="text-sm text-muted-foreground">{t("menus.others.none")}</p>
          )}
        </Section>
      </>
    );
  }

  return (
    <div data-slot="menus-screen" className="mx-auto flex w-full max-w-6xl flex-col gap-6">
      <PageHeader
        title={
          <span className="flex items-baseline gap-2.5">
            {t("menus.title")}
            {list.data ? (
              <span className="text-lg font-normal text-muted-foreground tabular-nums">
                {menus.length}
              </span>
            ) : null}
          </span>
        }
        description={t("menus.description")}
        actions={newButton("default")}
      />
      {body}
      <NewMenuDialog
        open={creating}
        onOpenChange={setCreating}
        taken={menus.map((m) => m.name)}
        onCreated={(menu) => {
          void qc.invalidateQueries({ queryKey: qk.menus.list });
          toast.success(t("menus.create.created", { label: menu.label || menu.name }));
          void navigate({ to: "/structure/menus/$name", params: { name: menu.name } });
        }}
      />
    </div>
  );
}

function Section({
  slot,
  title,
  hint,
  children,
}: {
  slot: string;
  title: string;
  hint: string;
  children: ReactNode;
}) {
  return (
    <section data-slot={slot} className="flex flex-col gap-3" aria-label={title}>
      <div className="flex flex-col gap-0.5">
        <h2 className="text-md font-medium text-foreground">{title}</h2>
        <p className="text-sm text-muted-foreground">{hint}</p>
      </div>
      {children}
    </section>
  );
}

function Row({
  slot,
  name,
  icon: Icon,
  title,
  detail,
  action,
}: {
  slot: string;
  name: string;
  icon: typeof ListTreeIcon;
  title: string;
  detail?: ReactNode;
  action: ReactNode;
}) {
  return (
    <li
      data-slot={slot}
      data-name={name}
      className="flex min-h-row flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border bg-card px-3 py-2 shadow-1"
    >
      <span className="flex size-9 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
        <Icon aria-hidden="true" className="size-4" />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex min-w-0 flex-wrap items-baseline gap-x-2">
          <span className="truncate font-medium text-foreground" title={title}>
            {title}
          </span>
          <code className="truncate font-mono text-xs text-muted-foreground" title={name}>
            {name}
          </code>
        </span>
        {detail ? <span className="text-sm text-muted-foreground">{detail}</span> : null}
      </div>
      <div className="ml-auto shrink-0">{action}</div>
    </li>
  );
}

function EditLink({ name, label }: { name: string; label: string }) {
  const { t } = useTranslation("structure");
  return (
    <Button asChild variant="outline">
      <Link
        to="/structure/menus/$name"
        params={{ name }}
        aria-label={t("menus.locations.editNamed", { label })}
      >
        <PencilIcon data-icon="inline-start" />
        {t("menus.locations.edit")}
      </Link>
    </Button>
  );
}

function ListSkeleton() {
  const { t } = useTranslation("structure");
  return (
    <div className="flex flex-col gap-2" role="status" aria-label={t("menus.loading")}>
      {[0, 1, 2].map((i) => (
        <Skeleton key={i} className="h-16 rounded-lg" />
      ))}
    </div>
  );
}

function NewMenuDialog({
  open,
  onOpenChange,
  taken,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  taken: string[];
  onCreated: (menu: MenuDto) => void;
}) {
  const { t } = useTranslation("structure");
  const [label, setLabel] = useState("");
  const [name, setName] = useState("");
  const [nameTyped, setNameTyped] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [serverErrors, setServerErrors] = useState<Record<string, string>>({});

  const reset = () => {
    setLabel("");
    setName("");
    setNameTyped(false);
    setAttempted(false);
    setServerErrors({});
  };
  const shownName = nameTyped ? name : nameFromMenuLabel(label);
  const problem = menuNameProblem(shownName);
  const nameError =
    serverErrors.name ??
    (taken.includes(shownName)
      ? t("menus.create.errors.taken", { name: shownName })
      : problem && (attempted || (nameTyped && problem !== "empty"))
        ? t(`menus.create.errors.${problem}`, { max: MAX_MENU_NAME })
        : undefined);
  const labelError =
    serverErrors.label ??
    ([...label].length > MAX_MENU_LABEL
      ? t("menus.create.errors.label", { max: MAX_MENU_LABEL })
      : undefined);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setAttempted(true);
    if (busy || problem !== null || taken.includes(shownName) || labelError) return;
    setBusy(true);
    try {
      const menu = await createMenu(shownName, label.trim());
      reset();
      onOpenChange(false);
      onCreated(menu);
    } catch (err) {
      reportApiError(err);
      if (err instanceof MenuExists) {
        setServerErrors({ name: t("menus.create.errors.taken", { name: shownName }) });
      } else if (err instanceof ApiError && err.status === 422) {
        const fields = err.fieldErrors();
        // Core names a bad menu name `slug` (its `slug::validate`), or gives no field.
        setServerErrors(
          fields.label
            ? { label: fields.label }
            : { name: fields.slug ?? fields.name ?? problemText(err) },
        );
      } else {
        toast.error(t("menus.create.failed"), { description: problemText(err) });
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) reset();
        onOpenChange(o);
      }}
    >
      <DialogContent data-slot="new-menu-dialog" className="sm:max-w-md">
        <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-5" noValidate>
          <DialogHeader>
            <DialogTitle>{t("menus.create.title")}</DialogTitle>
            <DialogDescription>{t("menus.create.description")}</DialogDescription>
          </DialogHeader>
          <FormField
            label={t("menus.create.label")}
            hint={t("menus.create.labelHint")}
            error={labelError}
          >
            {(control) => (
              <Input
                {...control}
                value={label}
                autoFocus
                disabled={busy}
                onChange={(e) => {
                  setLabel(e.currentTarget.value);
                  setServerErrors((errs) =>
                    nameTyped ? without(errs, "label") : without(without(errs, "label"), "name"),
                  );
                }}
              />
            )}
          </FormField>
          <FormField
            label={t("menus.create.name")}
            hint={t("menus.create.nameHint", { max: MAX_MENU_NAME })}
            error={nameError}
          >
            {(control) => (
              <Input
                {...control}
                value={shownName}
                className="font-mono"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                disabled={busy}
                onChange={(e) => {
                  setName(e.currentTarget.value);
                  setNameTyped(true);
                  setServerErrors((errs) => without(errs, "name"));
                }}
              />
            )}
          </FormField>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => {
                reset();
                onOpenChange(false);
              }}
            >
              {t("menus.create.cancel")}
            </Button>
            <Button type="submit" disabled={busy}>
              {t("menus.create.submit")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
