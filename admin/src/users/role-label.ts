import { useTranslation } from "react-i18next";
import { isRole } from "./users-search";

/** The role's name ("Editor"); an unknown role as sent. */
export function useRoleLabel(): (role: string) => string {
  const { t } = useTranslation("shell");
  return (role) => (isRole(role) ? t(`user.role.${role}`) : role);
}
