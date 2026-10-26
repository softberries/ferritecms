// Who uploaded a media item, as text: `MediaDto.author` (as content's `author`), its
// display name, else its login.

import type { MediaDto } from "@/api/media";

export function authorName(media: Pick<MediaDto, "author">): string {
  return media.author.display_name.trim() || media.author.login;
}
