// A media item a field or the featured image points at, for showing it.

import { useQuery } from "@tanstack/react-query";
import { qk } from "@/api/keys";
import { type MediaDto, getMedia } from "@/api/media";
import { ApiError } from "@/api/problem";

/** The media item `id`, cached like the library's (`null` when it is gone). */
export function useMediaItem(id: string | null) {
  return useQuery({
    // Under the item's key (refreshed with it), but its own entry: `null` for a gone item.
    queryKey: [...qk.media.item(id ?? ""), "ref"],
    queryFn: async (): Promise<MediaDto | null> => {
      try {
        const media = await getMedia(id ?? "");
        return media.status === "trash" ? null : media;
      } catch (e) {
        if (e instanceof ApiError && (e.status === 404 || e.status === 400)) return null;
        throw e;
      }
    },
    enabled: id !== null,
    staleTime: 60_000,
  });
}
