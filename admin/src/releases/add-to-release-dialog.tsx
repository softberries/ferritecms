// Add to release… and Save as change (design 15 §11.2): the item goes into an open release
// (or a new one named here) with a transition for when the release goes live (publish a
// draft, unpublish a public item, or keep its status) and, with "Include my changes", the
// editor's working draft as the staged change. Save as change (a public item of a workflow
// type, P7 D20) always stages the draft. An item already in the release has its entry
// replaced (`PUT`), else it is added (`POST`). Rights are the server's: a refusal is shown.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type FormEvent, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  type EntryTransition,
  MAX_RELEASE_TITLE,
  type ReleaseDetail,
  type StagedChange,
  addEntry,
  createRelease,
  getRelease,
  putEntry,
  releaseKeys,
  releasesQuery,
} from "@/api/releases";
import type { SchemaContentDto } from "@/api/schema";
import { Alert } from "@/auth/auth-layout";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { problemText } from "@/structure/taxonomies/terms-model";

export interface AddToReleaseDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  item: SchemaContentDto;
  /** The editor's working draft as a staged change (read when the dialog confirms). */
  staged: () => StagedChange;
  /** The editor holds unsaved changes ("Include my changes" starts checked). */
  dirty: boolean;
  /** Save as change: the draft is always staged. */
  asChange?: boolean;
  /** `included`: the working draft went into the release. */
  onAdded: (release: ReleaseDetail, included: boolean) => void;
}

const NEW = "new";

export function AddToReleaseDialog(props: AddToReleaseDialogProps) {
  return (
    <Dialog open={props.open} onOpenChange={props.onOpenChange}>
      <DialogContent
        data-slot="add-to-release-dialog"
        className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-lg"
      >
        {props.open ? <Body {...props} /> : null}
      </DialogContent>
    </Dialog>
  );
}

/** The transitions an item in `status` can get from a release. */
function transitionsFor(status: string): EntryTransition[] {
  if (status === "draft" || status === "pending") return ["none", "publish"];
  if (status === "published" || status === "private") return ["none", "unpublish"];
  return ["none"];
}

function Body({
  onOpenChange,
  item,
  staged,
  dirty,
  asChange = false,
  onAdded,
}: AddToReleaseDialogProps) {
  const { t } = useTranslation("releases");
  const queryClient = useQueryClient();
  const open = useQuery(releasesQuery("open"));
  const releases = open.data ?? [];
  const [picked, setPicked] = useState<string | null>(null);
  const choice = picked ?? (open.isSuccess ? (releases[0]?.id ?? NEW) : null);
  const [newTitle, setNewTitle] = useState("");
  const [transition, setTransition] = useState<EntryTransition>("none");
  const [include, setInclude] = useState(asChange || dirty);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const includeId = useId();
  const titleId = useId();
  const changes = asChange || include;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy || choice === null) return;
    if (!changes && transition === "none") {
      setError(t("add.nothing"));
      return;
    }
    if (choice === NEW && newTitle.trim() === "") {
      setError(t("newDialog.titleRequired"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const target =
        choice === NEW ? await createRelease({ title: newTitle.trim() }) : await getRelease(choice);
      const body = {
        transition,
        ...(changes ? { changes: staged() } : {}),
      };
      const next = target.entries.some((x) => x.item_id === item.id)
        ? await putEntry(target.id, item.id, target.version, body)
        : await addEntry(target.id, target.version, { item_id: item.id, ...body });
      queryClient.setQueryData(releaseKeys.item(next.id), next);
      void queryClient.invalidateQueries({ queryKey: releaseKeys.lists });
      onOpenChange(false);
      onAdded(next, changes);
    } catch (err) {
      setError(t("add.failed", { detail: problemText(err) }));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-5" noValidate>
      <DialogHeader>
        <DialogTitle>{asChange ? t("add.titleChange") : t("add.title")}</DialogTitle>
        <DialogDescription>{t("add.description")}</DialogDescription>
      </DialogHeader>
      <fieldset className="flex flex-col gap-2" disabled={busy}>
        <legend className="mb-2 text-sm font-medium">{t("add.release")}</legend>
        {open.isSuccess && releases.length === 0 ? (
          <p className="text-sm text-muted-foreground">{t("add.noneOpen")}</p>
        ) : null}
        <div className="flex flex-col gap-1.5">
          {releases.map((r) => (
            <Choice
              key={r.id}
              name="release"
              value={r.id}
              checked={choice === r.id}
              onSelect={() => setPicked(r.id)}
              label={r.title}
            />
          ))}
          <Choice
            name="release"
            value={NEW}
            checked={choice === NEW}
            onSelect={() => setPicked(NEW)}
            label={t("add.newRelease")}
          />
        </div>
        {choice === NEW ? (
          <div className="flex flex-col gap-1.5 pt-1">
            <Label htmlFor={titleId}>{t("add.newTitle")}</Label>
            <Input
              id={titleId}
              value={newTitle}
              maxLength={MAX_RELEASE_TITLE}
              onChange={(e) => setNewTitle(e.target.value)}
            />
          </div>
        ) : null}
      </fieldset>
      <fieldset className="flex flex-col gap-2" disabled={busy}>
        <legend className="mb-2 text-sm font-medium">{t("add.transition")}</legend>
        <div className="flex flex-col gap-1.5">
          {transitionsFor(item.status).map((tr) => (
            <Choice
              key={tr}
              name="transition"
              value={tr}
              checked={transition === tr}
              onSelect={() => setTransition(tr)}
              label={t(`add.transitions.${tr}`)}
            />
          ))}
        </div>
      </fieldset>
      {asChange ? (
        <p className="rounded-md bg-muted/60 px-3 py-2 text-sm text-muted-foreground">
          {t("add.includeForced")}
        </p>
      ) : (
        <div className="flex items-start gap-2">
          <Checkbox
            id={includeId}
            checked={include}
            disabled={busy}
            onCheckedChange={(c) => setInclude(c === true)}
            className="mt-0.5"
            aria-describedby={`${includeId}-hint`}
          />
          <div className="flex flex-col gap-0.5">
            <Label htmlFor={includeId}>{t("add.include")}</Label>
            <p id={`${includeId}-hint`} className="text-xs text-muted-foreground">
              {t("add.includeHint")}
            </p>
          </div>
        </div>
      )}
      {error ? <Alert tone="error">{error}</Alert> : null}
      <DialogFooter>
        <Button type="button" variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
          {t("add.cancel")}
        </Button>
        <Button type="submit" data-slot="add-to-release-confirm" disabled={busy || choice === null}>
          {busy ? t("add.adding") : asChange ? t("add.confirmChange") : t("add.confirm")}
        </Button>
      </DialogFooter>
    </form>
  );
}

function Choice({
  name,
  value,
  checked,
  onSelect,
  label,
}: {
  name: string;
  value: string;
  checked: boolean;
  onSelect: () => void;
  label: string;
}) {
  return (
    <label className="flex min-h-11 cursor-pointer items-center gap-3 rounded-lg border px-3 text-sm has-checked:border-primary has-checked:bg-primary/6 has-focus-visible:ring-2 has-focus-visible:ring-ring">
      <input
        type="radio"
        name={name}
        value={value}
        checked={checked}
        onChange={onSelect}
        className="size-4 accent-primary"
      />
      <span className="min-w-0 truncate">{label}</span>
    </label>
  );
}
