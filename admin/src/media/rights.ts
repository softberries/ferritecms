// What the viewer may do to a media item, for showing actions only (Global Constraints:
// `MediaDto` carries no `allowed`, P30). The server checks every call again, and a `403`
// is shown as an error, never assumed away. The rules mirror 07 §1 "Media": upload needs
// `upload_media`; edit, trash and delete the content role rules (own item, else
// `edit_others`); restore also needs `publish_own`/`publish_others` (it republishes).

import type { MediaDto } from "@/api/media";
import type { SchemaAllowedActionsDto } from "@/api/schema";
import type { MeDto } from "@/auth/session";

export function canUpload(me: MeDto): boolean {
  return me.capabilities.includes("upload_media");
}

export function canEditMedia(me: MeDto, media: Pick<MediaDto, "author_id">): boolean {
  return media.author_id === me.id
    ? me.capabilities.includes("edit_own")
    : me.capabilities.includes("edit_others");
}

export function canRestoreMedia(me: MeDto, media: Pick<MediaDto, "author_id">): boolean {
  const publish = media.author_id === me.id ? "publish_own" : "publish_others";
  return canEditMedia(me, media) && me.capabilities.includes(publish);
}

/** A media item as `runBulk` and the bulk bar read it: the content `allowed` shape. */
export interface MediaBulkItem {
  id: string;
  allowed: SchemaAllowedActionsDto;
  media: MediaDto;
}

export function bulkItem(me: MeDto, media: MediaDto): MediaBulkItem {
  const trashed = media.status === "trash";
  const edit = canEditMedia(me, media);
  const restore = canRestoreMedia(me, media);
  return {
    id: media.id,
    media,
    allowed: {
      edit: edit && !trashed,
      trash: edit && !trashed,
      trash_undoable: edit && !trashed && restore,
      restore: trashed && restore,
      delete: trashed && edit,
      publish: false,
      unpublish: false,
      schedule: false,
      submit: false,
    },
  };
}
