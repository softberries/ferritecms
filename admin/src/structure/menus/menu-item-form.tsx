// Editing one menu entry in the draft (07 §3 Menus): its label (empty: the site shows the
// target's title, or a custom link's address) and, for a custom link, its address under
// core's rules (`validMenuUrl`). Nothing is sent: the change lands in the draft, which
// Save writes as a whole.

import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
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
import { returnToRowMenu } from "../taxonomies/row-focus";
import { FormField } from "../types/form-field";
import {
  MAX_MENU_LABEL,
  MAX_MENU_URL,
  type MenuEntry,
  type MenuNode,
  validMenuUrl,
} from "./menu-model";

export interface MenuItemDialogProps {
  /** The entry being edited; `null` closes the dialog. */
  node: MenuNode | null;
  /** What the entry links to, as its row shows it (title, or "Page · About"). */
  target: string;
  onApply: (id: string, entry: MenuEntry) => void;
  onClose: () => void;
}

export function MenuItemDialog({ node, target, onApply, onClose }: MenuItemDialogProps) {
  const { t } = useTranslation("structure");
  return (
    <Dialog open={node !== null} onOpenChange={(o) => (o ? undefined : onClose())}>
      {node ? (
        <DialogContent
          data-slot="menu-item-dialog"
          className="sm:max-w-md"
          onCloseAutoFocus={returnToRowMenu(node.id)}
        >
          <DialogHeader>
            <DialogTitle>{t("menus.item.title")}</DialogTitle>
            <DialogDescription>{t("menus.item.description")}</DialogDescription>
          </DialogHeader>
          {/* Keyed by the entry, so each opening starts from its values. */}
          <ItemForm
            key={node.id}
            node={node}
            target={target}
            onApply={(entry) => {
              onApply(node.id, entry);
              onClose();
            }}
            onCancel={onClose}
          />
        </DialogContent>
      ) : null}
    </Dialog>
  );
}

function ItemForm({
  node,
  target,
  onApply,
  onCancel,
}: {
  node: MenuNode;
  target: string;
  onApply: (entry: MenuEntry) => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation("structure");
  const isLink = node.data.kind === "url";
  const [label, setLabel] = useState(node.data.label);
  const [url, setUrl] = useState(node.data.url ?? "");
  const [attempted, setAttempted] = useState(false);
  const labelError =
    [...label].length > MAX_MENU_LABEL
      ? t("menus.editor.problems.labelTooLong", { max: MAX_MENU_LABEL })
      : undefined;
  const urlError =
    isLink && attempted && !validMenuUrl(url.trim()) ? t("menus.add.urlInvalid") : undefined;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setAttempted(true);
    if (labelError || (isLink && !validMenuUrl(url.trim()))) return;
    onApply({ ...node.data, label, url: isLink ? url.trim() : null });
  };

  return (
    <form onSubmit={submit} className="flex flex-col gap-5" noValidate>
      {isLink ? (
        <FormField label={t("menus.item.url")} hint={t("menus.add.urlHint")} error={urlError}>
          {(control) => (
            <Input
              {...control}
              value={url}
              type="url"
              inputMode="url"
              autoFocus
              maxLength={MAX_MENU_URL + 20}
              className="font-mono"
              spellCheck={false}
              onChange={(e) => setUrl(e.currentTarget.value)}
            />
          )}
        </FormField>
      ) : (
        <div className="flex min-w-0 flex-col gap-1">
          <span className="text-sm font-medium">{t("menus.item.target")}</span>
          <span className="truncate text-sm text-muted-foreground" title={target}>
            {target}
          </span>
        </div>
      )}
      <FormField
        label={t("menus.item.label")}
        hint={isLink ? t("menus.item.labelHintLink") : t("menus.item.labelHint", { title: target })}
        error={labelError}
      >
        {(control) => (
          <Input
            {...control}
            value={label}
            autoFocus={!isLink}
            placeholder={isLink ? url : target}
            onChange={(e) => setLabel(e.currentTarget.value)}
          />
        )}
      </FormField>
      <DialogFooter>
        <Button type="button" variant="outline" onClick={onCancel}>
          {t("menus.item.cancel")}
        </Button>
        <Button type="submit">{t("menus.item.apply")}</Button>
      </DialogFooter>
    </form>
  );
}
