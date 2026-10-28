// Small pieces the users list, the sheet and the dialogs share: the avatar, role, status
// and two-factor badges, and a user's name with their login. Names and logins are the
// users' own text and render as text.

import { cn } from "cn";
import { ShieldCheckIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { UserDto } from "@/api/users";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { initials } from "@/lib/initials";
import { useRoleLabel } from "./role-label";

export function UserAvatar({
  user,
  size = "default",
  className,
}: {
  user: Pick<UserDto, "display_name" | "login">;
  size?: "default" | "lg";
  className?: string;
}) {
  return (
    <Avatar size={size} className={className} aria-hidden="true">
      <AvatarFallback
        className={cn(
          "bg-primary/12 font-semibold text-foreground",
          size === "lg" ? "text-sm" : "text-xs",
        )}
      >
        {initials(user.display_name || user.login)}
      </AvatarFallback>
    </Avatar>
  );
}

export function RoleBadge({ role }: { role: string }) {
  const label = useRoleLabel();
  return (
    <Badge variant={role === "admin" ? "secondary" : "outline"} data-slot="role-badge">
      {label(role)}
    </Badge>
  );
}

/** Active, Disabled, and "Must change password" for an active `must_reset` account. */
export function UserStatusBadge({ user }: { user: Pick<UserDto, "status" | "must_reset"> }) {
  const { t } = useTranslation("admin");
  if (user.status === "disabled") {
    return (
      <Badge variant="destructive" data-slot="status-badge">
        {t("users.status.disabled")}
      </Badge>
    );
  }
  return (
    <span className="inline-flex items-center gap-1.5 text-sm" data-slot="status-badge">
      <span aria-hidden="true" className="size-2 shrink-0 rounded-full bg-status-published" />
      {user.must_reset ? t("users.status.mustReset") : t("users.status.active")}
    </span>
  );
}

/** "2FA" when the user has two-factor authentication on; nothing otherwise. */
export function TwoFactorBadge({ on, className }: { on: boolean; className?: string }) {
  const { t } = useTranslation("admin");
  if (!on) return null;
  return (
    <Badge
      variant="outline"
      className={className}
      title={t("users.twoFactorOn")}
      data-slot="two-factor-badge"
    >
      <ShieldCheckIcon data-icon="inline-start" aria-hidden="true" />
      <span aria-hidden="true">{t("users.twoFactorShort")}</span>
      <span className="sr-only">{t("users.twoFactorOn")}</span>
    </Badge>
  );
}

/** Display name over the login, both truncated with the full text in `title`. */
export function UserName({
  user,
  className,
}: {
  user: Pick<UserDto, "display_name" | "login">;
  className?: string;
}) {
  return (
    <span className={cn("flex min-w-0 flex-col", className)}>
      <span className="truncate font-medium text-foreground" title={user.display_name}>
        {user.display_name}
      </span>
      <span className="truncate font-mono text-xs text-muted-foreground" title={user.login}>
        {user.login}
      </span>
    </span>
  );
}
