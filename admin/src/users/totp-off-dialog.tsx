// Turn off another user's two-factor authentication (`POST /users/{id}/totp/disable`,
// P43: the lost-authenticator path), after a confirm. Their sessions and tokens stay.

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { type UserDto, disableUserTotp } from "@/api/users";
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

/** `POST /users/{id}/totp/disable` (P43), after a confirm. */
export function TotpOffDialog({
  user,
  onClose,
  onDone,
}: {
  user: UserDto | null;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation("admin");
  const [shown, setShown] = useState(user);
  if (user && user !== shown) setShown(user);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  const run = async () => {
    if (!user || pending) return;
    setPending(true);
    setFailure(null);
    try {
      await disableUserTotp(user.id);
      toast.success(t("users.totp.done", { name: user.display_name }));
      onDone();
    } catch (error) {
      reportApiError(error);
      setFailure(problemText(error));
    } finally {
      setPending(false);
    }
  };

  return (
    <AlertDialog
      open={user !== null}
      onOpenChange={(open) => {
        if (!open && !pending) {
          setFailure(null);
          onClose();
        }
      }}
    >
      <AlertDialogContent data-slot="totp-off">
        <AlertDialogHeader>
          <AlertDialogTitle className="break-words">
            {t("users.totp.title", { name: shown?.display_name ?? "" })}
          </AlertDialogTitle>
          <AlertDialogDescription>{t("users.totp.body")}</AlertDialogDescription>
        </AlertDialogHeader>
        {failure ? <Alert tone="error">{failure}</Alert> : null}
        <AlertDialogFooter>
          <Button variant="outline" onClick={onClose} disabled={pending}>
            {t("users.cancel")}
          </Button>
          <Button
            className={DESTRUCTIVE}
            onClick={() => void run()}
            disabled={pending}
            aria-busy={pending || undefined}
          >
            {t("users.totp.confirm")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
