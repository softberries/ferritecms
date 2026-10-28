// Deleting a user (`DELETE /users/{id}`): a confirm that says what goes with the account
// (sessions end, API tokens are revoked; H-A4). The server refuses a user who owns
// content with a `422` whose field is `reassign` (P46; there is no count endpoint, so the
// first try finds out): the same dialog then asks who takes the content over, from the
// other active users (`GET /users?status=active&q=…`), and tries again with
// `?reassign=<id>`. Any other refusal (oneself, the last admin) shows its detail here.

import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { SearchIcon } from "lucide-react";
import { useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { INLINE_ERRORS, reportApiError } from "@/api/client";
import { ApiError } from "@/api/problem";
import { type UserDto, deleteUser, listUsers } from "@/api/users";
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
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { Skeleton } from "@/components/ui/skeleton";
import { DESTRUCTIVE } from "@/lib/destructive";
import { problemText } from "@/structure/taxonomies/terms-model";
import { UserAvatar, UserName } from "./user-parts";

/** The picker's search waits this long after the last key (as the list's search). */
export const PICKER_DEBOUNCE_MS = 250;
/** Users the picker shows at once; a search narrows them. */
const PICKER_LIMIT = 20;

export interface DeleteUserDialogProps {
  /** `null`: closed. */
  user: UserDto | null;
  onClose: () => void;
  onDeleted: (user: UserDto) => void;
}

export function DeleteUserDialog({ user, onClose, onDeleted }: DeleteUserDialogProps) {
  // The last user asked about, so the text stays while the dialog closes; a new key per
  // user, so the steps start again.
  const [shown, setShown] = useState(user);
  if (user && user !== shown) setShown(user);
  const [busy, setBusy] = useState(false);
  return (
    <AlertDialog open={user !== null} onOpenChange={(open) => !open && !busy && onClose()}>
      <AlertDialogContent data-slot="delete-user" className="sm:max-w-lg">
        {shown ? (
          <DeleteSteps
            key={shown.id}
            user={shown}
            busy={busy}
            setBusy={setBusy}
            onClose={onClose}
            onDeleted={onDeleted}
          />
        ) : null}
      </AlertDialogContent>
    </AlertDialog>
  );
}

function DeleteSteps({
  user,
  busy,
  setBusy,
  onClose,
  onDeleted,
}: {
  user: UserDto;
  busy: boolean;
  setBusy: (busy: boolean) => void;
  onClose: () => void;
  onDeleted: (user: UserDto) => void;
}) {
  const { t } = useTranslation("admin");
  const [reassigning, setReassigning] = useState(false);
  const [heir, setHeir] = useState<UserDto | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const run = async () => {
    if (busy || (reassigning && !heir)) return;
    setBusy(true);
    setFailure(null);
    try {
      await deleteUser(user.id, reassigning ? heir?.id : undefined);
      onDeleted(user);
    } catch (error) {
      reportApiError(error);
      const fields = error instanceof ApiError && error.status === 422 ? error.fieldErrors() : {};
      if (fields.reassign !== undefined && !reassigning) {
        setReassigning(true);
        return;
      }
      setFailure(fields.reassign ?? problemText(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <AlertDialogHeader>
        <AlertDialogTitle className="break-words">
          {t("users.delete.title", { name: user.display_name })}
        </AlertDialogTitle>
        <AlertDialogDescription>{t("users.delete.body")}</AlertDialogDescription>
      </AlertDialogHeader>
      {reassigning ? (
        <div data-slot="reassign" className="flex min-w-0 flex-col gap-3">
          <p className="text-sm font-medium text-foreground">{t("users.delete.reassign")}</p>
          <UserPicker exclude={user.id} value={heir} onChange={setHeir} disabled={busy} />
          <p className="text-xs text-muted-foreground">{t("users.delete.reassignHint")}</p>
        </div>
      ) : null}
      {failure ? <Alert tone="error">{failure}</Alert> : null}
      <AlertDialogFooter>
        <Button variant="outline" onClick={onClose} disabled={busy}>
          {t("users.cancel")}
        </Button>
        <Button
          className={DESTRUCTIVE}
          onClick={() => void run()}
          disabled={busy || (reassigning && !heir)}
          aria-busy={busy}
        >
          {busy
            ? t("users.delete.deleting")
            : reassigning
              ? t("users.delete.confirmReassign")
              : t("users.delete.confirm")}
        </Button>
      </AlertDialogFooter>
    </>
  );
}

/**
 * Picks one active user other than `exclude`: a search box (the server's `q`, after
 * `PICKER_DEBOUNCE_MS`) over a list of native radios.
 */
function UserPicker({
  exclude,
  value,
  onChange,
  disabled,
}: {
  exclude: string;
  value: UserDto | null;
  onChange: (user: UserDto) => void;
  disabled: boolean;
}) {
  const { t } = useTranslation("admin");
  const id = useId();
  const [text, setText] = useState("");
  const [q, setQ] = useState("");
  useEffect(() => {
    const timer = window.setTimeout(() => setQ(text.trim()), PICKER_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [text]);
  const list = useQuery({
    queryKey: ["users", "list", "picker", q],
    queryFn: ({ signal }) =>
      listUsers({ status: "active", limit: PICKER_LIMIT, ...(q ? { q } : {}) }, { signal }),
    placeholderData: keepPreviousData,
    // The picker shows its own error.
    meta: INLINE_ERRORS,
  });
  const users = (list.data?.items ?? []).filter((u) => u.id !== exclude);

  return (
    <div className="flex min-w-0 flex-col gap-2">
      <InputGroup>
        <InputGroupAddon>
          <SearchIcon />
        </InputGroupAddon>
        <InputGroupInput
          type="search"
          value={text}
          aria-label={t("users.delete.search")}
          placeholder={t("users.delete.searchPlaceholder")}
          autoComplete="off"
          spellCheck={false}
          disabled={disabled}
          onChange={(e) => setText(e.currentTarget.value)}
          className="[&::-webkit-search-cancel-button]:hidden"
        />
      </InputGroup>
      <div
        role="radiogroup"
        aria-label={t("users.delete.pickLabel")}
        className="flex max-h-56 min-h-14 flex-col gap-0.5 overflow-y-auto rounded-lg border p-1"
      >
        {list.isPending ? (
          <div aria-busy="true" className="flex flex-col gap-1 p-1">
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
          </div>
        ) : list.isError ? (
          <p role="alert" className="px-2 py-3 text-sm text-destructive">
            {problemText(list.error)}
          </p>
        ) : users.length === 0 ? (
          <p className="px-2 py-3 text-sm text-muted-foreground">
            {q ? t("users.delete.noMatch", { q }) : t("users.delete.nobody")}
          </p>
        ) : (
          users.map((u) => {
            const inputId = `${id}-${u.id}`;
            return (
              <label
                key={u.id}
                htmlFor={inputId}
                className="relative flex min-h-11 cursor-pointer items-center gap-3 rounded-md px-2 py-1.5 has-checked:bg-primary/10 has-focus-visible:ring-2 has-focus-visible:ring-ring hover:bg-muted"
              >
                <input
                  id={inputId}
                  type="radio"
                  name={`${id}-heir`}
                  value={u.id}
                  checked={value?.id === u.id}
                  disabled={disabled}
                  onChange={() => onChange(u)}
                  className="size-4 shrink-0 accent-primary focus-visible:outline-none"
                />
                <UserAvatar user={u} />
                <UserName user={u} />
              </label>
            );
          })
        )}
      </div>
    </div>
  );
}
