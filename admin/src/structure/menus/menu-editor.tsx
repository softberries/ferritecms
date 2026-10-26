// One menu's editor (07 §3 Menus; `manage_menus`): Add items and the menu's label on the
// left, the menu as a `SortableTree` on the right (one column on phones). Every change is
// a draft: rows move by drag, by keyboard or from their menu (Edit, the moves, Remove with
// Undo), and nothing is sent until Save (`mod+s`), which checks the draft against core's
// limits (`checkMenu`), re-reads the menu (`staleGuard`, Review Focus 4: someone else's
// change asks Reload / Overwrite) and `PUT`s the whole tree. Discard drops the draft;
// leaving with one asks first. Delete menu removes it, and its Undo re-`PUT`s the copy
// deleted.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useRouter } from "@tanstack/react-router";
import {
  CircleAlertIcon,
  FileTextIcon,
  LinkIcon,
  ListTreeIcon,
  PencilIcon,
  TagIcon,
  Trash2Icon,
  Undo2Icon,
  XIcon,
} from "lucide-react";
import { useMemo, useRef, useState } from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { qk } from "@/api/keys";
import { type MenuDto, deleteMenu, getMenu, menusQuery, putMenu } from "@/api/menus";
import { ApiError } from "@/api/problem";
import { EmptyState } from "@/components/kit/empty-state";
import { PageHeader } from "@/components/kit/page-header";
import { SaveState, type SaveStateValue } from "@/components/kit/save-state";
import { SortableTree } from "@/components/kit/sortable-tree";
import { type FlatNode, flatten } from "@/components/kit/tree-model";
import { useStaleAsk } from "@/components/kit/use-stale-ask";
import { UNDO_TOAST_MS } from "@/content/list-actions";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { useLeaveGuard } from "@/editor/leave-guard";
import { useShortcut } from "@/lib/shortcuts";
import { stableEqual, staleGuard } from "@/lib/stale-guard";
import { problemText } from "../taxonomies/terms-model";
import { DESTRUCTIVE } from "../shared";
import { FormField } from "../types/form-field";
import { AddItems } from "./add-items";
import {
  MAX_MENU_ITEMS,
  MAX_MENU_LABEL,
  MENU_MAX_DEPTH,
  type MenuEntry,
  type MenuNode,
  type MenuProblem,
  checkMenu,
  fromDto,
  newNode,
  removeEntry,
  restoreEntry,
  toInput,
} from "./menu-model";
import { MenuItemDialog } from "./menu-item-form";
import { type Target, useTargets } from "./targets";

const MODAL = '[data-slot="dialog-overlay"], [data-slot="alert-dialog-overlay"]';
const KIND_ICON = { item: FileTextIcon, term: TagIcon, url: LinkIcon } as const;

/** The server's copy as the `PUT` body would carry it (for comparing drafts and copies). */
const normal = (menu: MenuDto | null) => menu && toInput(fromDto(menu), menu.label);

const is404 = (e: unknown) => e instanceof ApiError && e.status === 404;

export function MenuEditor({ initial }: { initial: MenuDto }) {
  const { t } = useTranslation("structure");
  const qc = useQueryClient();
  const navigate = useNavigate();
  const router = useRouter();
  const list = useQuery(menusQuery);
  const [base, setBase] = useState(initial);
  const [label, setLabel] = useState(initial.label);
  const [nodes, setNodes] = useState<MenuNode[]>(() => fromDto(initial));
  const [attempted, setAttempted] = useState(false);
  const [saving, setSaving] = useState(false);
  const [savedAt, setSavedAt] = useState<Date | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleting, setDeleting] = useState(false);
  // The Undo toasts of entries removed from this draft (gone once it is saved or dropped).
  const removals = useRef(new Set<string | number>());
  const stale = useStaleAsk(base.label || base.name);
  const targets = useTargets(nodes);

  const location = list.data?.locations.find((l) => l.name === base.name);
  const draft = useMemo(() => toInput(nodes, label), [nodes, label]);
  const dirty = !stableEqual(draft, normal(base));
  const problems = useMemo(() => checkMenu(nodes), [nodes]);
  const labelTooLong = [...label].length > MAX_MENU_LABEL;
  const problemsById = useMemo(() => {
    const out = new Map<string, MenuProblem[]>();
    for (const p of problems) {
      if (p.kind === "tooMany") continue;
      out.set(p.id, [...(out.get(p.id) ?? []), p]);
    }
    return out;
  }, [problems]);

  const forgetRemovals = () => {
    for (const id of removals.current) toast.dismiss(id);
    removals.current.clear();
  };
  const reset = (dto: MenuDto) => {
    forgetRemovals();
    setBase(dto);
    setLabel(dto.label);
    setNodes(fromDto(dto));
    setAttempted(false);
    setFailure(null);
  };

  const nameOf = (n: MenuNode): string => {
    if (n.data.label.trim()) return n.data.label;
    if (n.data.kind === "url") return n.data.url ?? "";
    const target = targets.get(n.id);
    return target?.state === "found" ? target.title : t(`menus.editor.kind.${n.data.kind}`);
  };

  const save = async () => {
    if (saving || deleting) return;
    setAttempted(true);
    setFailure(null);
    if (problems.length > 0 || labelTooLong) {
      toast.error(t("menus.editor.invalid"));
      requestAnimationFrame(() =>
        document
          .querySelector("[data-slot=menu-problem], [data-slot=menu-label] [aria-invalid=true]")
          ?.scrollIntoView({ block: "center" }),
      );
      return;
    }
    if (!dirty) return;
    setSaving(true);
    try {
      const guard = await staleGuard<MenuDto | null>({
        reread: async () => {
          try {
            return await getMenu(base.name);
          } catch (e) {
            if (is404(e)) return null;
            throw e;
          }
        },
        base,
        equal: (a, b) => stableEqual(normal(a), normal(b)),
        ask: stale.ask,
      });
      if (!guard.proceed) {
        if (guard.reloaded === null) {
          forgetRemovals();
          toast.info(t("menus.editor.goneElsewhere", { label: base.label || base.name }));
          void qc.invalidateQueries({ queryKey: qk.menus.list });
          void navigate({ to: "/structure/menus", ignoreBlocker: true });
        } else if (guard.reloaded) {
          reset(guard.reloaded);
          toast.info(t("menus.editor.reloaded"));
        }
        return;
      }
      const saved = await putMenu(base.name, draft);
      forgetRemovals();
      setBase(saved);
      setLabel(saved.label);
      setAttempted(false);
      setSavedAt(new Date());
      void qc.invalidateQueries({ queryKey: qk.menus.list });
      // The crumb and the document title follow a new label.
      void router.invalidate();
      toast.success(t("menus.editor.saved", { label: saved.label || saved.name }));
    } catch (e) {
      const message = problemText(e);
      setFailure(message);
      toast.error(t("menus.editor.notSaved"), { description: message });
    } finally {
      setSaving(false);
    }
  };

  const discard = () => {
    reset(base);
    toast.info(t("menus.editor.discarded"));
  };

  const add = (entries: MenuEntry[]) => setNodes((ns) => [...ns, ...entries.map(newNode)]);

  const remove = (node: FlatNode<MenuEntry>) => {
    const flat = flatten(nodes);
    const at = flat.findIndex((n) => n.id === node.id);
    const result = removeEntry(nodes, node.id);
    setNodes(result.nodes);
    // Focus goes to the row now in its place (or the one before), as it would in a list.
    const neighbour = flat[at + 1]?.id ?? flat[at - 1]?.id ?? null;
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        if (neighbour === null) return;
        document.querySelector<HTMLElement>(`[data-row-menu="${CSS.escape(neighbour)}"]`)?.focus();
      }),
    );
    const name = nameOf(node);
    const children = result.removed.childIds.length;
    const id = toast(
      children > 0
        ? t("menus.editor.removedChildren", { label: name, count: children })
        : t("menus.editor.removed", { label: name }),
      {
        duration: UNDO_TOAST_MS,
        action: {
          label: t("menus.editor.undo"),
          onClick: () => {
            removals.current.delete(id);
            setNodes((ns) => restoreEntry(ns, result.removed));
            toast.success(t("menus.editor.restored", { label: name }));
          },
        },
      },
    );
    removals.current.add(id);
  };

  const removeMenu = async () => {
    setConfirmDelete(false);
    setDeleting(true);
    const snapshot = base;
    const name = snapshot.label || snapshot.name;
    try {
      await deleteMenu(snapshot.name);
      forgetRemovals();
      void qc.invalidateQueries({ queryKey: qk.menus.list });
      toast.success(t("menus.delete.deleted", { label: name }), {
        duration: UNDO_TOAST_MS,
        action: {
          label: t("menus.editor.undo"),
          onClick: () => {
            putMenu(snapshot.name, toInput(fromDto(snapshot), snapshot.label))
              .then(() => {
                void qc.invalidateQueries({ queryKey: qk.menus.list });
                toast.success(t("menus.editor.restored", { label: name }));
              })
              .catch((e: unknown) =>
                toast.error(t("menus.delete.undoFailed", { label: name }), {
                  description: problemText(e),
                }),
              );
          },
        },
      });
      void navigate({ to: "/structure/menus", ignoreBlocker: true });
    } catch (e) {
      toast.error(t("menus.delete.failed"), { description: problemText(e) });
    } finally {
      setDeleting(false);
    }
  };

  const leave = useLeaveGuard(dirty && !deleting);
  useShortcut({
    id: "menus.save",
    keys: "mod+s",
    label: t("menus.editor.shortcuts.save"),
    group: t("menus.editor.shortcuts.group"),
    allowInInput: true,
    run: () => {
      if (document.querySelector(MODAL) !== null) return;
      void save();
    },
  });

  const state: SaveStateValue | null = saving
    ? "saving"
    : failure !== null
      ? "error"
      : dirty
        ? "dirty"
        : savedAt
          ? "saved"
          : null;
  const editingNode = nodes.find((n) => n.id === editing) ?? null;
  // The tree names rows by their data; each entry object belongs to one node.
  const names = new Map(nodes.map((n) => [n.data, nameOf(n)]));
  const treeName = t("menus.editor.treeLabel", { label: label || base.name });

  return (
    <div data-slot="menu-editor" className="mx-auto flex w-full max-w-6xl flex-col gap-6">
      <PageHeader
        title={
          <span className="block min-w-0 truncate" title={base.label || base.name}>
            {base.label || t("menus.unlabelled")}
          </span>
        }
        description={
          <span className="flex flex-wrap items-center gap-2">
            <code className="font-mono text-sm text-muted-foreground">{base.name}</code>
            {location ? (
              <Badge variant="secondary">
                {t("menus.editor.location", { location: location.label || location.name })}
              </Badge>
            ) : list.data ? (
              <span className="text-sm text-muted-foreground">{t("menus.editor.noLocation")}</span>
            ) : null}
          </span>
        }
        actions={
          <>
            {state ? <SaveState state={state} {...(savedAt ? { at: savedAt } : {})} /> : null}
            <Button
              variant="outline"
              disabled={deleting || saving}
              onClick={() => setConfirmDelete(true)}
              className="text-destructive hover:border-destructive/50 hover:text-destructive"
            >
              <Trash2Icon data-icon="inline-start" />
              {t("menus.delete.button")}
            </Button>
            {dirty ? (
              <Button variant="outline" disabled={saving} onClick={discard}>
                <Undo2Icon data-icon="inline-start" />
                {t("menus.editor.discard")}
              </Button>
            ) : null}
            <Button onClick={() => void save()} disabled={!dirty || saving || deleting}>
              {saving ? t("menus.editor.saving") : t("menus.editor.save")}
            </Button>
          </>
        }
      />

      <div className="grid items-start gap-6 lg:grid-cols-[22rem_minmax(0,1fr)] lg:grid-rows-[auto_1fr]">
        <Card data-slot="menu-add" className="min-w-0 lg:col-start-1 lg:row-start-1">
          <CardHeader>
            <CardTitle role="heading" aria-level={2} className="text-md">
              {t("menus.add.title")}
            </CardTitle>
            <CardDescription>{t("menus.add.description")}</CardDescription>
          </CardHeader>
          <CardContent>
            <AddItems onAdd={add} room={MAX_MENU_ITEMS - nodes.length} disabled={saving} />
          </CardContent>
        </Card>

        <Card
          data-slot="menu-items"
          className="min-w-0 lg:col-start-2 lg:row-span-2 lg:row-start-1"
        >
          <CardHeader>
            <CardTitle role="heading" aria-level={2} className="flex items-baseline gap-2 text-md">
              {t("menus.editor.items")}
              <span className="text-sm font-normal text-muted-foreground tabular-nums">
                {t("menus.editor.itemCount", { count: nodes.length })}
              </span>
            </CardTitle>
            <CardDescription>{t("menus.editor.treeHint")}</CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            {failure ? <Alert slot="save-failure">{failure}</Alert> : null}
            {attempted && problems.some((p) => p.kind === "tooMany") ? (
              <Alert slot="menu-problem">
                {t("menus.editor.problems.tooMany", { max: MAX_MENU_ITEMS, count: nodes.length })}
              </Alert>
            ) : null}
            {nodes.length === 0 ? (
              <EmptyState
                icon={ListTreeIcon}
                title={t("menus.editor.empty.title")}
                body={t("menus.editor.empty.body")}
                className="py-10"
              />
            ) : (
              <SortableTree
                nodes={nodes}
                name={treeName}
                maxDepth={MENU_MAX_DEPTH}
                label={(entry) => names.get(entry) || t(`menus.editor.kind.${entry.kind}`)}
                onChange={(next) => setNodes(next)}
                render={(node) => (
                  <MenuRow
                    node={node}
                    target={targets.get(node.id)}
                    problems={attempted ? (problemsById.get(node.id) ?? []) : []}
                    onEdit={() => setEditing(node.id)}
                  />
                )}
                leadingActions={(node) => [
                  {
                    key: "edit",
                    label: t("menus.editor.edit"),
                    icon: PencilIcon,
                    disabled: saving,
                    overlay: true,
                    run: () => setEditing(node.id),
                  },
                ]}
                trailingActions={(node) => [
                  {
                    key: "remove",
                    label: t("menus.editor.remove"),
                    icon: XIcon,
                    disabled: saving,
                    separated: true,
                    run: () => remove(node),
                  },
                ]}
              />
            )}
          </CardContent>
        </Card>

        <Card data-slot="menu-settings" className="min-w-0 lg:col-start-1 lg:row-start-2">
          <CardHeader>
            <CardTitle role="heading" aria-level={2} className="text-md">
              {t("menus.editor.settings")}
            </CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-5">
            <FormField
              slot="menu-label"
              label={t("menus.editor.label")}
              hint={t("menus.editor.labelHint")}
              error={
                labelTooLong
                  ? t("menus.editor.problems.labelTooLong", { max: MAX_MENU_LABEL })
                  : undefined
              }
            >
              {(control) => (
                <Input
                  {...control}
                  value={label}
                  disabled={saving}
                  onChange={(e) => setLabel(e.currentTarget.value)}
                />
              )}
            </FormField>
            <div className="flex flex-col gap-1">
              <span className="text-sm font-medium">{t("menus.editor.name")}</span>
              <code className="font-mono text-sm break-all">{base.name}</code>
              <p className="text-xs text-muted-foreground">{t("menus.editor.nameFixed")}</p>
            </div>
          </CardContent>
        </Card>
      </div>

      <MenuItemDialog
        node={editingNode}
        target={editingNode ? targetText(editingNode, targets.get(editingNode.id), t) : ""}
        onApply={(id, entry) =>
          setNodes((ns) => ns.map((n) => (n.id === id ? { ...n, data: entry } : n)))
        }
        onClose={() => setEditing(null)}
      />
      {stale.dialog}
      <AlertDialog open={confirmDelete} onOpenChange={(o) => setConfirmDelete(o)}>
        <AlertDialogContent data-slot="delete-menu-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("menus.delete.confirmTitle", { label: base.label || base.name })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {location
                ? `${t("menus.delete.confirmLocation", { location: location.label || location.name })} `
                : null}
              {t("menus.delete.confirmBody")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("menus.delete.cancel")}</AlertDialogCancel>
            <Button variant="destructive" className={DESTRUCTIVE} onClick={() => void removeMenu()}>
              {t("menus.delete.confirm")}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <AlertDialog open={leave.blocked} onOpenChange={(o) => (o ? undefined : leave.stay())}>
        <AlertDialogContent data-slot="menu-leave-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("menus.editor.leave.title")}</AlertDialogTitle>
            <AlertDialogDescription>{t("menus.editor.leave.body")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("menus.editor.leave.stay")}</AlertDialogCancel>
            <Button variant="destructive" className={DESTRUCTIVE} onClick={leave.leave}>
              {t("menus.editor.leave.leave")}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

type T = TFunction<"structure">;

/** "Page · About us", "Category · News", or a custom link's address. */
function targetText(node: MenuNode, target: Target | undefined, t: T): string {
  if (node.data.kind === "url") return node.data.url ?? "";
  if (target?.state === "found")
    return `${target.kind} · ${target.title || t("menus.add.untitled")}`;
  if (target?.state === "missing") return t("menus.editor.targetMissing");
  if (target?.state === "failed") return t("menus.editor.targetFailed");
  return t("menus.editor.targetLoading");
}

function MenuRow({
  node,
  target,
  problems,
  onEdit,
}: {
  node: FlatNode<MenuEntry>;
  target: Target | undefined;
  problems: MenuProblem[];
  onEdit: () => void;
}) {
  const { t } = useTranslation("structure");
  const Icon = KIND_ICON[node.data.kind];
  const own = node.data.label.trim();
  const fallback =
    node.data.kind === "url"
      ? (node.data.url ?? "")
      : target?.state === "found"
        ? target.title || t("menus.add.untitled")
        : "";
  const sub = targetText(node, target, t);
  // A found target names its own type or taxonomy ("Pages · About"); the rest say the kind.
  const line =
    target?.state === "found" ? sub : `${t(`menus.editor.kind.${node.data.kind}`)} · ${sub}`;
  const broken = target?.state === "missing";
  return (
    <div className="flex min-w-0 items-center gap-2.5 py-0.5" data-slot="menu-entry">
      <Icon
        aria-hidden="true"
        className={
          broken ? "size-4 shrink-0 text-destructive" : "size-4 shrink-0 text-muted-foreground"
        }
      />
      <div className="flex min-w-0 flex-1 flex-col">
        <button
          type="button"
          onClick={onEdit}
          className="max-w-full min-w-0 self-start truncate rounded-sm text-left font-medium text-foreground hover:underline"
          title={own || fallback}
          data-slot="menu-entry-label"
        >
          {own ? own : <span className="font-normal text-muted-foreground">{fallback || sub}</span>}
        </button>
        <span
          className={
            broken ? "truncate text-xs text-destructive" : "truncate text-xs text-muted-foreground"
          }
          title={line}
          data-slot="menu-entry-target"
        >
          {line}
        </span>
        {problems.map((p) => (
          <span key={p.kind} data-slot="menu-problem" className="text-xs text-destructive">
            {p.kind === "tooDeep"
              ? t("menus.editor.problems.tooDeep", { max: MENU_MAX_DEPTH + 1 })
              : p.kind === "labelTooLong"
                ? t("menus.editor.problems.labelTooLong", { max: MAX_MENU_LABEL })
                : t("menus.editor.problems.badUrl")}
          </span>
        ))}
      </div>
    </div>
  );
}

function Alert({ slot, children }: { slot: string; children: string }) {
  return (
    <p
      role="alert"
      data-slot={slot}
      className="flex items-start gap-2 rounded-lg border border-destructive/40 bg-destructive/8 px-3 py-2 text-sm text-foreground"
    >
      <CircleAlertIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-destructive" />
      {children}
    </p>
  );
}
