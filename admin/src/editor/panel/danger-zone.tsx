// The danger zone: Move to trash, shown when `allowed.trash`. It reuses the list's trash
// (Task 13: the Undo toast re-applies the old status, F36; a trash nothing could undo
// asks first). Unsaved changes are saved before the trash (a trashed item takes no
// edits), then the editor goes back to the list, where the Undo toast stays.

import { useNavigate } from "@tanstack/react-router";
import { Trash2Icon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { trashUndoable } from "@/content/bulk";
import { useListActions } from "@/content/list-actions";
import type { EditorDocument } from "../editor-state";

export function DangerZone({ doc, type }: { doc: EditorDocument; type: string }) {
  const { t } = useTranslation("editor");
  const { t: tc } = useTranslation("content");
  const navigate = useNavigate();
  const item = doc.item;
  const [asking, setAsking] = useState(false);
  const [saving, setSaving] = useState(false);
  const actions = useListActions(type, {
    onDone: (ok) => {
      if (item && ok.includes(item.id)) {
        void navigate({ to: `/content/${type}` as "/", ignoreBlocker: true });
      }
    },
  });
  if (!item || !item.allowed.trash || doc.readOnly !== null) return null;
  const undoable = trashUndoable([item]);

  const trash = async () => {
    setAsking(false);
    setSaving(true);
    try {
      // The changes first: they come back with an Undo.
      const saved = doc.dirty ? await doc.save() : item;
      if (saved) actions.run("trash", [saved]);
    } finally {
      setSaving(false);
    }
  };

  return (
    <section
      aria-labelledby="panel-danger"
      data-slot="panel-danger"
      className="flex flex-col gap-2"
    >
      <h3 id="panel-danger" className="text-sm font-medium">
        {t("danger.heading")}
      </h3>
      <p className="text-xs text-muted-foreground">
        {undoable ? t("danger.hintUndo") : t("danger.hintNoUndo")}
      </p>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={saving || actions.pending !== null}
        className="self-start text-destructive hover:bg-destructive/10 hover:text-destructive"
        onClick={() => {
          if (undoable) void trash();
          else setAsking(true);
        }}
      >
        <Trash2Icon data-icon="inline-start" />
        {tc("actions.trash")}
      </Button>
      <AlertDialog open={asking} onOpenChange={setAsking}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{tc("trash.title", { count: 1 })}</AlertDialogTitle>
            <AlertDialogDescription>
              {tc("trash.bodyOne", { title: doc.title || tc("row.untitled") })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{tc("delete.cancel")}</AlertDialogCancel>
            <Button
              variant="destructive"
              className="bg-destructive text-background hover:bg-destructive/90 dark:bg-destructive dark:hover:bg-destructive/90"
              onClick={() => {
                void trash();
              }}
            >
              {tc("trash.confirm", { count: 1 })}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}
