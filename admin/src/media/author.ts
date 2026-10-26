// Who uploaded a media item, as text. `MediaDto` carries only `author_id` (no author
// object, unlike content), and `GET /users/{id}` needs `manage_users` for another user
// (07 §1), so: "You" for the viewer's own items, the user's display name for viewers
// with `manage_users` (one cached read per user), else a neutral "Another user".

import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { INLINE_ERRORS, api, unwrap } from "@/api/client";
import { qk } from "@/api/keys";
import type { MeDto } from "@/auth/session";

export function useAuthorName(me: MeDto, authorId: string): string {
  const { t } = useTranslation("media");
  const own = authorId === me.id;
  const lookup = !own && me.capabilities.includes("manage_users");
  const user = useQuery({
    queryKey: qk.user(authorId),
    queryFn: () => unwrap(api.GET("/api/v1/users/{id}", { params: { path: { id: authorId } } })),
    enabled: lookup,
    staleTime: 5 * 60_000,
    meta: INLINE_ERRORS,
  });
  if (own) return t("author.you");
  const name = user.data ? user.data.display_name.trim() || user.data.login : "";
  return name || t("author.other");
}
