// A row's hover actions (07 §3): quick Edit and Trash/Restore at `md`+, and the full
// menu. Every item comes from the item's `allowed` (Review Focus 4), except Preview,
// which any item outside the trash has (`/_preview/<id>` reads whatever the list shows).

import { Link } from "@tanstack/react-router";
import {
  ExternalLinkIcon,
  MoreHorizontalIcon,
  PencilIcon,
  RotateCcwIcon,
  Trash2Icon,
  XCircleIcon,
} from "lucide-react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { type ContentSummary, type RowAction, editPath } from "./list-columns";

export interface RowActionsProps {
  type: string;
  item: ContentSummary;
  onOpen: (item: ContentSummary) => void;
  onAction: (action: RowAction, item: ContentSummary) => void;
}

export function RowActions({ type, item, onOpen, onAction }: RowActionsProps) {
  const { t } = useTranslation("content");
  const title = item.title || t("row.untitled");
  const { allowed } = item;
  const trashed = item.status === "trash";
  const hasMenu = allowed.edit || !trashed || allowed.restore || allowed.trash || allowed.delete;
  const quick = (label: string, icon: ReactNode, onClick: () => void) => (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className="max-md:hidden"
          aria-label={t("row.actionOn", { action: label, title })}
          onClick={onClick}
        >
          {icon}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
  return (
    <>
      {allowed.edit ? quick(t("actions.edit"), <PencilIcon />, () => onOpen(item)) : null}
      {allowed.trash
        ? quick(t("actions.trash"), <Trash2Icon />, () => onAction("trash", item))
        : null}
      {allowed.restore
        ? quick(t("actions.restore"), <RotateCcwIcon />, () => onAction("restore", item))
        : null}
      {hasMenu ? (
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              className="pointer-coarse:size-control"
              aria-label={t("row.menu", { title })}
            >
              <MoreHorizontalIcon />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-52">
            {allowed.edit ? (
              <DropdownMenuItem asChild>
                <Link to={editPath(type, item.id) as "/"}>
                  <PencilIcon />
                  {t("actions.edit")}
                </Link>
              </DropdownMenuItem>
            ) : null}
            {!trashed ? (
              <DropdownMenuItem asChild>
                <a href={`/_preview/${item.id}`} target="_blank" rel="noopener noreferrer">
                  <ExternalLinkIcon />
                  {t("actions.preview")}
                  <span className="sr-only">{t("row.newTab")}</span>
                </a>
              </DropdownMenuItem>
            ) : null}
            {allowed.restore ? (
              <DropdownMenuItem onSelect={() => onAction("restore", item)}>
                <RotateCcwIcon />
                {t("actions.restore")}
              </DropdownMenuItem>
            ) : null}
            {allowed.trash || allowed.delete ? <DropdownMenuSeparator /> : null}
            {allowed.trash ? (
              <DropdownMenuItem variant="destructive" onSelect={() => onAction("trash", item)}>
                <Trash2Icon />
                {t("actions.trash")}
              </DropdownMenuItem>
            ) : null}
            {allowed.delete ? (
              <DropdownMenuItem variant="destructive" onSelect={() => onAction("delete", item)}>
                <XCircleIcon />
                {t("actions.delete")}
              </DropdownMenuItem>
            ) : null}
          </DropdownMenuContent>
        </DropdownMenu>
      ) : null}
    </>
  );
}
