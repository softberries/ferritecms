// The confirm of a role or status change in the user sheet: it says what the change ends
// (H-A4: a role change ends their sessions and keeps their tokens, limited by the new
// role; a disable ends sessions and revokes tokens). The server's refusal (the last
// admin, H-A2) shows in the dialog with its detail.

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { reportApiError } from "@/api/client";
import type { RoleDto, UserDto, UserStatus } from "@/api/users";
import { Alert } from "@/auth/auth-layout";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { DESTRUCTIVE } from "@/lib/destructive";
import { problemText } from "@/structure/taxonomies/terms-model";
import { useRoleLabel } from "./role-label";

/** A pending role or status change of the sheet's user. */
export type AccessChange = { kind: "role"; value: RoleDto } | { kind: "status"; value: UserStatus };

/** The confirm of a role or status change; says what it ends (H-A4). */
export function AccessDialog({
  user,
  change,
  onClose,
  apply,
}: {
  user: UserDto;
  change: AccessChange | null;
  onClose: () => void;
  apply: (change: AccessChange) => Promise<void>;
}) {
  const { t } = useTranslation("admin");
  const roleLabel = useRoleLabel();
  const [shown, setShown] = useState(change);
  if (change && change !== shown) setShown(change);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [opened, setOpened] = useState(change);
  if (change !== opened) {
    setOpened(change);
    setFailure(null);
  }
  const name = user.display_name;
  let title = "";
  let body = "";
  let confirm = "";
  let destructive = false;
  if (shown?.kind === "role") {
    title = t("users.access.roleTitle", { name, role: roleLabel(shown.value) });
    body = t("users.access.roleBody");
    confirm = t("users.access.roleConfirm");
  } else if (shown?.value === "disabled") {
    title = t("users.access.disableTitle", { name });
    body = t("users.access.disableBody");
    confirm = t("users.access.disableConfirm");
    destructive = true;
  } else if (shown) {
    title = t("users.access.enableTitle", { name });
    body = t("users.access.enableBody");
    confirm = t("users.access.enableConfirm");
  }

  const run = async () => {
    if (!change || pending) return;
    setPending(true);
    setFailure(null);
    try {
      await apply(change);
    } catch (error) {
      reportApiError(error);
      setFailure(problemText(error));
    } finally {
      setPending(false);
    }
  };

  return (
    <AlertDialog open={change !== null} onOpenChange={(open) => !open && !pending && onClose()}>
      <AlertDialogContent data-slot="access-change">
        <AlertDialogHeader>
          <AlertDialogTitle className="break-words">{title}</AlertDialogTitle>
          <AlertDialogDescription>{body}</AlertDialogDescription>
        </AlertDialogHeader>
        {failure ? <Alert tone="error">{failure}</Alert> : null}
        <AlertDialogFooter>
          <Button variant="outline" onClick={onClose} disabled={pending}>
            {t("users.cancel")}
          </Button>
          <Button
            className={destructive ? DESTRUCTIVE : undefined}
            onClick={() => void run()}
            disabled={pending}
            aria-busy={pending || undefined}
          >
            {confirm}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
