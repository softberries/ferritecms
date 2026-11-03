// A content type's approval workflow (M6-3, design 15 §6.1, §11.4; admins). Steps as a
// sortable list (`SortableList`, @dnd-kit): each with a label, approvers by role (editors,
// admins) and by name (active editors and admins), and how many approvals it needs (1–5);
// up to 5 steps. The preset "One editor approves" fills in one step. Saved on its own
// (`PUT /workflows/{type}`, `If-Match` once one exists); open reviews keep the steps they
// started with. Removing the workflow asks first: its open reviews are withdrawn.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { PlusIcon, Trash2Icon, WorkflowIcon } from "lucide-react";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  MAX_REQUIRED,
  MAX_STEPS,
  MAX_STEP_USERS,
  type WorkflowDto,
  type WorkflowStep,
  deleteWorkflow,
  keyFor,
  oneEditorApproves,
  putWorkflow,
  stepIssues,
  workflowKeys,
  workflowQuery,
} from "@/api/reviews";
import { listUsers } from "@/api/users";
import { SortableList } from "@/components/kit/sortable-list";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { problemText } from "@/structure/taxonomies/terms-model";

interface Row {
  /** A local id (the list's key). */
  id: string;
  step: WorkflowStep;
  /** Added here: its key follows its label when saved. */
  fresh: boolean;
}

let rowSeq = 0;
const rowOf = (step: WorkflowStep, fresh: boolean): Row => ({
  id: `step-${++rowSeq}`,
  step: {
    ...step,
    approvers: {
      roles: [...(step.approvers.roles ?? [])],
      users: [...(step.approvers.users ?? [])],
    },
  },
  fresh,
});

/** The steps to save: fresh steps get a key from their label. */
function stepsToSave(rows: readonly Row[]): WorkflowStep[] {
  const taken = new Set(rows.filter((r) => !r.fresh).map((r) => r.step.key));
  return rows.map((r) => {
    const label = r.step.label.trim();
    const approvers = { roles: r.step.approvers.roles ?? [], users: r.step.approvers.users ?? [] };
    if (!r.fresh) return { ...r.step, label, approvers };
    const key = keyFor(label, taken);
    taken.add(key);
    return { ...r.step, key, label, approvers };
  });
}

export function WorkflowEditor({ type }: { type: string }) {
  const { t } = useTranslation("reviews");
  const query = useQuery(workflowQuery(type));
  return (
    <Card data-slot="type-workflow">
      <section aria-labelledby={`workflow-${type}`} className="contents">
        <CardHeader>
          <CardTitle
            id={`workflow-${type}`}
            role="heading"
            aria-level={2}
            className="flex items-center gap-2 text-md"
          >
            <WorkflowIcon aria-hidden="true" className="size-4 text-muted-foreground" />
            {t("workflow.title")}
          </CardTitle>
          <CardDescription>{t("workflow.description")}</CardDescription>
        </CardHeader>
        <CardContent>
          {query.isPending ? (
            <Skeleton className="h-24 w-full" />
          ) : (
            <Editor key={query.data?.version ?? 0} type={type} workflow={query.data ?? null} />
          )}
        </CardContent>
      </section>
    </Card>
  );
}

function Editor({ type, workflow }: { type: string; workflow: WorkflowDto | null }) {
  const { t } = useTranslation("reviews");
  const queryClient = useQueryClient();
  const [rows, setRows] = useState<Row[]>(() =>
    (workflow?.steps ?? []).map((s) => rowOf(s, false)),
  );
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const users = useQuery({
    queryKey: ["users", "list", "approvers"],
    staleTime: 60_000,
    queryFn: async () => {
      const [editors, admins] = await Promise.all([
        listUsers({ role: "editor", status: "active", limit: 200 }),
        listUsers({ role: "admin", status: "active", limit: 200 }),
      ]);
      return [...editors.items, ...admins.items];
    },
  });
  const issues = rows.length === 0 ? {} : stepIssues(rows.map((r) => r.step));

  const change = (next: Row[]) => {
    setRows(next);
    setDirty(true);
  };
  const update = (id: string, patch: Partial<WorkflowStep>) =>
    change(rows.map((r) => (r.id === id ? { ...r, step: { ...r.step, ...patch } } : r)));

  const take = (next: WorkflowDto | null) => {
    queryClient.setQueryData(workflowKeys.item(type), next);
  };

  const save = async () => {
    setBusy(true);
    try {
      take(await putWorkflow(type, stepsToSave(rows), workflow?.version ?? null));
      toast.success(t("workflow.saved"), { id: "workflow" });
    } catch (e) {
      toast.error(t("workflow.failed", { detail: problemText(e) }), { id: "workflow" });
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!workflow) return;
    setBusy(true);
    try {
      await deleteWorkflow(type, workflow.version);
      take(null);
      toast.success(t("workflow.deleted"), { id: "workflow" });
    } catch (e) {
      toast.error(t("workflow.failed", { detail: problemText(e) }), { id: "workflow" });
    } finally {
      setBusy(false);
      setConfirm(false);
    }
  };

  const addStep = () => {
    const label = t("workflow.newStep");
    change([
      ...rows,
      rowOf({ key: "step", label, approvers: { roles: [], users: [] }, required: 1 }, true),
    ]);
  };

  return (
    <div className="flex flex-col gap-4">
      {rows.length === 0 ? (
        <p className="text-sm text-muted-foreground">{t("workflow.none")}</p>
      ) : (
        <SortableList
          items={rows}
          getId={(r) => r.id}
          label={(r) => r.step.label || t("workflow.newStep")}
          name={t("workflow.steps")}
          onReorder={change}
          rowClassName="items-start"
          render={(r) => {
            const i = rows.indexOf(r);
            return (
              <StepFields
                row={r}
                issues={issues}
                index={i}
                users={users.data ?? []}
                disabled={busy}
                onChange={(patch) => update(r.id, patch)}
                onRemove={() => change(rows.filter((x) => x.id !== r.id))}
              />
            );
          }}
        />
      )}
      <div className="flex flex-wrap items-center gap-2">
        {rows.length === 0 ? (
          <Button
            type="button"
            variant="outline"
            disabled={busy}
            onClick={() =>
              change(oneEditorApproves(t("workflow.presetLabel")).map((s) => rowOf(s, false)))
            }
          >
            {t("workflow.preset")}
          </Button>
        ) : null}
        <Button
          type="button"
          variant="outline"
          disabled={busy || rows.length >= MAX_STEPS}
          onClick={addStep}
        >
          <PlusIcon data-icon="inline-start" />
          {t("workflow.addStep")}
        </Button>
        <span className="flex-1" />
        {workflow ? (
          <Button
            type="button"
            variant="ghost"
            className="text-destructive hover:text-destructive"
            disabled={busy}
            onClick={() => setConfirm(true)}
          >
            <Trash2Icon data-icon="inline-start" />
            {t("workflow.delete")}
          </Button>
        ) : null}
        <Button
          type="button"
          disabled={busy || !dirty || rows.length === 0 || Object.keys(issues).length > 0}
          onClick={() => void save()}
        >
          {busy ? t("workflow.saving") : t("workflow.save")}
        </Button>
      </div>
      <AlertDialog open={confirm} onOpenChange={setConfirm}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("workflow.deleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("workflow.deleteBody")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("workflow.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={(e) => {
                e.preventDefault();
                void remove();
              }}
            >
              {t("workflow.deleteConfirm")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function StepFields({
  row,
  index,
  issues,
  users,
  disabled,
  onChange,
  onRemove,
}: {
  row: Row;
  index: number;
  issues: Record<string, string>;
  users: { id: string; login: string; display_name: string }[];
  disabled: boolean;
  onChange: (patch: Partial<WorkflowStep>) => void;
  onRemove: () => void;
}) {
  const { t } = useTranslation("reviews");
  const id = useId();
  const s = row.step;
  const roles = s.approvers.roles ?? [];
  const chosen = s.approvers.users ?? [];
  const issue = (member: string) => {
    const code = issues[`${index}.${member}`];
    return code ? t(`workflow.issues.${code as "label"}`) : undefined;
  };
  const labelError = issue("label");
  const approverError = issue("approvers");
  const requiredError = issue("required");
  const setRole = (role: "editor" | "admin", on: boolean) =>
    onChange({
      approvers: {
        roles: on ? [...roles.filter((r) => r !== role), role] : roles.filter((r) => r !== role),
        users: chosen,
      },
    });
  return (
    <div data-slot="workflow-step" className="flex min-w-0 flex-1 flex-col gap-3 py-1">
      <div className="flex flex-wrap items-end gap-3">
        <div className="flex min-w-48 flex-1 flex-col gap-1.5">
          <Label htmlFor={`${id}-label`}>{t("workflow.label")}</Label>
          <Input
            id={`${id}-label`}
            value={s.label}
            maxLength={80}
            disabled={disabled}
            aria-invalid={labelError ? true : undefined}
            aria-describedby={labelError ? `${id}-label-error` : undefined}
            onChange={(e) => onChange({ label: e.target.value })}
          />
        </div>
        <div className="flex w-36 flex-col gap-1.5">
          <Label htmlFor={`${id}-required`}>{t("workflow.required")}</Label>
          <Input
            id={`${id}-required`}
            type="number"
            min={1}
            max={MAX_REQUIRED}
            value={Number.isNaN(s.required) ? "" : s.required}
            disabled={disabled}
            aria-invalid={requiredError ? true : undefined}
            aria-describedby={requiredError ? `${id}-required-error` : undefined}
            onChange={(e) =>
              onChange({ required: e.target.value === "" ? NaN : Number(e.target.value) })
            }
            className="tabular-nums"
          />
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          aria-label={t("workflow.remove", { label: s.label })}
          disabled={disabled}
          onClick={onRemove}
          className="text-muted-foreground"
        >
          <Trash2Icon />
        </Button>
      </div>
      {labelError ? (
        <p id={`${id}-label-error`} className="-mt-1.5 text-xs text-destructive">
          {labelError}
        </p>
      ) : null}
      {requiredError ? (
        <p id={`${id}-required-error`} className="-mt-1.5 text-xs text-destructive">
          {requiredError}
        </p>
      ) : null}
      <fieldset className="flex flex-col gap-2">
        <legend className="mb-1 text-sm font-medium">{t("workflow.roles")}</legend>
        <div className="flex flex-wrap gap-4">
          {(["editor", "admin"] as const).map((role) => (
            <div key={role} className="flex min-h-11 items-center gap-2 sm:min-h-0">
              <Checkbox
                id={`${id}-${role}`}
                checked={roles.includes(role)}
                disabled={disabled}
                onCheckedChange={(c) => setRole(role, c === true)}
              />
              <Label htmlFor={`${id}-${role}`} className="font-normal">
                {t(`workflow.${role}`)}
              </Label>
            </div>
          ))}
        </div>
      </fieldset>
      {users.length > 0 ? (
        <fieldset className="flex flex-col gap-2">
          <legend className="mb-1 text-sm font-medium">{t("workflow.users")}</legend>
          <p className="text-xs text-muted-foreground">{t("workflow.usersHint")}</p>
          <div className="flex flex-wrap gap-x-4 gap-y-1">
            {users.map((u) => {
              const on = chosen.includes(u.id);
              return (
                <div key={u.id} className="flex min-h-11 items-center gap-2 sm:min-h-8">
                  <Checkbox
                    id={`${id}-u-${u.id}`}
                    checked={on}
                    disabled={disabled || (!on && chosen.length >= MAX_STEP_USERS)}
                    onCheckedChange={(c) =>
                      onChange({
                        approvers: {
                          roles,
                          users: c === true ? [...chosen, u.id] : chosen.filter((x) => x !== u.id),
                        },
                      })
                    }
                  />
                  <Label htmlFor={`${id}-u-${u.id}`} className="font-normal">
                    {u.display_name || u.login}
                  </Label>
                </div>
              );
            })}
          </div>
        </fieldset>
      ) : null}
      {approverError ? <p className="text-xs text-destructive">{approverError}</p> : null}
    </div>
  );
}
