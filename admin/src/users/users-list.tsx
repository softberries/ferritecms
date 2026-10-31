// The users screen (07 §3 Users; `manage_users`): every account by login, 50 a page with
// Load more; search (`q`, in the URL, 250 ms after the last key, `/` focuses it) over
// login, display name and e-mail; role and status chips (in the URL); New user in a
// dialog (`?new=true`, the palette's "New user"); a row opens its user in a sheet
// (`?id=`). Display names, logins and e-mails are the users' own text and render as text.

import { keepPreviousData, useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import { cn } from "cn";
import { ChevronRightIcon, PlusIcon, SearchXIcon, UsersIcon } from "lucide-react";
import { type ReactNode, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { qk } from "@/api/keys";
import { USERS_PAGE, type UserDto, listUsers } from "@/api/users";
import type { MeDto } from "@/auth/session";
import { DataTable } from "@/components/kit/data-table";
import { type DataTableColumn, dataTableColumns } from "@/components/kit/data-table-features";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { RelativeTime } from "@/components/kit/relative-time";
import { ScrollFade } from "@/components/kit/scroll-fade";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Chip, SearchField } from "@/content/filters";
import { NewUserDialog } from "./new-user-dialog";
import { useRoleLabel } from "./role-label";
import { RoleBadge, TwoFactorBadge, UserAvatar, UserStatusBadge } from "./user-parts";
import { UserSheet } from "./user-sheet";
import { ROLES, STATUSES, type UsersSearch } from "./users-search";

/** The search waits this long after the last key (constraints: exact values). */
export const USERS_SEARCH_DEBOUNCE_MS = 250;

export interface UsersScreenProps {
  search: UsersSearch;
  setSearch: (patch: Partial<Record<keyof UsersSearch, UsersSearch[keyof UsersSearch]>>) => void;
  me: MeDto;
}

export function UsersScreen({ search, setSearch, me }: UsersScreenProps) {
  const { t } = useTranslation("admin");
  const roleLabel = useRoleLabel();
  const queryClient = useQueryClient();
  const filters = useMemo(
    () => ({
      ...(search.q ? { q: search.q } : {}),
      ...(search.role ? { role: search.role } : {}),
      ...(search.status ? { status: search.status } : {}),
    }),
    [search.q, search.role, search.status],
  );
  const list = useInfiniteQuery({
    queryKey: qk.usersInfinite(filters),
    queryFn: ({ pageParam, signal }) =>
      listUsers(
        { ...filters, limit: USERS_PAGE, ...(pageParam ? { cursor: pageParam } : {}) },
        { signal },
      ),
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    placeholderData: keepPreviousData,
  });
  const users = useMemo(() => {
    const seen = new Map<string, UserDto>();
    for (const page of list.data?.pages ?? []) {
      for (const u of page.items) if (!seen.has(u.id)) seen.set(u.id, u);
    }
    return [...seen.values()];
  }, [list.data]);
  const columns = useColumns(me);
  const filtered = Boolean(search.q || search.role || search.status);
  const refresh = () => void queryClient.invalidateQueries({ queryKey: qk.usersLists });
  const open = (u: UserDto) => setSearch({ id: u.id });

  let empty: ReactNode;
  if (filtered) {
    empty = (
      <EmptyState
        icon={SearchXIcon}
        title={t("users.noMatch.title")}
        body={search.q ? t("users.noMatch.bodyQ", { q: search.q }) : t("users.noMatch.body")}
        action={
          <Button
            variant="outline"
            onClick={() => setSearch({ q: undefined, role: undefined, status: undefined })}
          >
            {t("users.clearFilters")}
          </Button>
        }
        className="border-0"
      />
    );
  } else {
    // Never reached in practice: the viewer is a user too.
    empty = (
      <EmptyState
        icon={UsersIcon}
        title={t("users.empty.title")}
        body={t("users.empty.body")}
        className="border-0"
      />
    );
  }

  return (
    <div data-slot="users-screen" className="mx-auto flex w-full max-w-6xl flex-col gap-5">
      <PageHeader
        title={t("users.title")}
        description={t("users.description")}
        actions={
          <Button onClick={() => setSearch({ new: true })}>
            <PlusIcon data-icon="inline-start" />
            {t("users.new.action")}
          </Button>
        }
      />
      <div role="search" className="flex flex-col gap-3 lg:flex-row lg:items-center">
        <SearchField
          label={t("users.searchLabel")}
          value={search.q ?? ""}
          debounceMs={USERS_SEARCH_DEBOUNCE_MS}
          onSearch={(q) => setSearch({ q: q || undefined })}
        />
        {/* Phones: one row that scrolls sideways; wider screens wrap. */}
        <ScrollFade className="-m-1 min-w-0 p-1 lg:overflow-visible">
          <div className="flex w-max items-center gap-2 lg:w-auto lg:flex-wrap">
            <div role="group" aria-label={t("users.filterRole")} className="flex gap-2">
              {ROLES.map((r) => (
                <Chip
                  key={r}
                  pressed={search.role === r}
                  onClick={() => setSearch({ role: search.role === r ? undefined : r })}
                >
                  {roleLabel(r)}
                </Chip>
              ))}
            </div>
            <span aria-hidden="true" className="mx-1 h-5 w-px shrink-0 bg-border" />
            <div role="group" aria-label={t("users.filterStatus")} className="flex gap-2">
              {STATUSES.map((s) => (
                <Chip
                  key={s}
                  pressed={search.status === s}
                  onClick={() => setSearch({ status: search.status === s ? undefined : s })}
                >
                  {t(`users.status.${s}`)}
                </Chip>
              ))}
            </div>
            {filtered ? (
              <Button
                variant="ghost"
                className="text-muted-foreground max-lg:order-first"
                onClick={() => setSearch({ q: undefined, role: undefined, status: undefined })}
              >
                {t("users.clearFilters")}
              </Button>
            ) : null}
          </div>
        </ScrollFade>
      </div>
      {list.isError && !list.data ? (
        <ErrorState error={list.error} retry={() => void list.refetch()} />
      ) : (
        <DataTable
          label={t("users.title")}
          columns={columns}
          data={users}
          getRowId={(u) => u.id}
          onRowOpen={open}
          loading={list.isPending}
          animateRows
          rowActions={(u) => (
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={t("users.openOn", { name: u.display_name })}
              title={t("users.open")}
              onClick={() => open(u)}
            >
              <ChevronRightIcon />
            </Button>
          )}
          columnClassName={{
            name: "w-full max-w-0 md:w-[30%]",
            email: "hidden max-w-0 md:table-cell md:w-[28%]",
            role: "hidden sm:table-cell",
            status: "hidden lg:table-cell",
            two_factor: "hidden lg:table-cell",
            last_login: "hidden whitespace-nowrap xl:table-cell",
          }}
          className={
            list.isPlaceholderData ? "opacity-60 transition-opacity" : "transition-opacity"
          }
          empty={empty}
          footer={
            list.hasNextPage ? (
              <div className="flex justify-center border-t px-4 py-3">
                <Button
                  variant="ghost"
                  disabled={list.isFetchingNextPage}
                  aria-busy={list.isFetchingNextPage || undefined}
                  onClick={() => {
                    if (!list.isFetchingNextPage) void list.fetchNextPage();
                  }}
                >
                  {list.isFetchingNextPage ? t("users.loadingMore") : t("users.loadMore")}
                </Button>
              </div>
            ) : null
          }
        />
      )}
      <NewUserDialog
        open={search.new === true}
        onOpenChange={(o) => setSearch({ new: o ? true : undefined })}
        me={me}
        onCreated={(created, note) => {
          setSearch({ new: undefined });
          refresh();
          toast.success(t("users.new.created", { name: created.display_name }), {
            ...(note ? { description: note } : {}),
          });
        }}
      />
      {search.id ? (
        <UserSheet
          key={search.id}
          id={search.id}
          me={me}
          onClose={() => setSearch({ id: undefined })}
          onDeleted={(gone) => {
            setSearch({ id: undefined });
            refresh();
            toast.success(t("users.delete.done", { name: gone.display_name }));
          }}
        />
      ) : null}
    </div>
  );
}

function useColumns(me: MeDto): DataTableColumn<UserDto>[] {
  const { t } = useTranslation("admin");
  return useMemo(() => {
    const col = dataTableColumns<UserDto>();
    return [
      col.accessor("display_name", {
        id: "name",
        header: () => t("users.columns.user"),
        cell: (c) => {
          const u = c.row.original;
          return (
            <div className="flex min-w-0 items-center gap-3 py-2">
              <UserAvatar user={u} />
              <div className="flex min-w-0 flex-col">
                <span className="flex min-w-0 items-center gap-2">
                  <span className="truncate font-medium text-foreground" title={u.display_name}>
                    {u.display_name}
                  </span>
                  {u.id === me.id ? (
                    <Badge variant="outline" className="shrink-0">
                      {t("users.you")}
                    </Badge>
                  ) : null}
                </span>
                <span className="truncate font-mono text-xs text-muted-foreground" title={u.login}>
                  {u.login}
                </span>
                {/* Below `lg` the Status and Two-factor columns are hidden, below `sm` the
                    Role column too: their badges go here, each up to where its column shows. */}
                <span
                  data-slot="name-extra"
                  className={cn(
                    "mt-1 flex flex-wrap items-center gap-2",
                    // With only the role to show, the row goes with the role column.
                    u.status === "disabled" || u.must_reset || u.totp_enabled
                      ? "lg:hidden"
                      : "sm:hidden",
                  )}
                >
                  <span className="contents sm:hidden">
                    <RoleBadge role={u.role} />
                  </span>
                  {u.status === "disabled" || u.must_reset ? <UserStatusBadge user={u} /> : null}
                  <TwoFactorBadge on={u.totp_enabled} />
                </span>
              </div>
            </div>
          );
        },
      }),
      col.accessor("email", {
        id: "email",
        header: () => t("users.columns.email"),
        cell: (c) => (
          <span className="block truncate text-muted-foreground" title={c.row.original.email}>
            {c.row.original.email}
          </span>
        ),
      }),
      col.accessor("role", {
        id: "role",
        header: () => t("users.columns.role"),
        cell: (c) => <RoleBadge role={c.row.original.role} />,
      }),
      col.accessor("status", {
        id: "status",
        header: () => t("users.columns.status"),
        cell: (c) => <UserStatusBadge user={c.row.original} />,
      }),
      col.accessor("totp_enabled", {
        id: "two_factor",
        header: () => t("users.columns.twoFactor"),
        cell: (c) =>
          c.row.original.totp_enabled ? (
            <TwoFactorBadge on />
          ) : (
            <span className="text-sm text-muted-foreground">{t("users.twoFactorOff")}</span>
          ),
      }),
      col.accessor("last_login_at", {
        id: "last_login",
        header: () => t("users.columns.lastLogin"),
        cell: (c) =>
          c.row.original.last_login_at ? (
            <RelativeTime at={c.row.original.last_login_at} className="text-muted-foreground" />
          ) : (
            <span className="text-muted-foreground">{t("users.never")}</span>
          ),
      }),
    ];
  }, [t, me.id]);
}
