// The media files this test uploaded, so the guard can tell a 404 of another test's file
// (accepted in the media specs: their library lists every item on the site, and a
// parallel test may purge one) from a 404 of this test's own file (a real failure).
//
// A stored file's URL is `/_media/<sha256>/<name>`, where `<name>` is the item's original
// name; its variants share that name under their own hashes (03 §5). Every upload in the
// specs has a name unique to its test (`uid`), so the name identifies the test's files,
// variants included, before the variants exist.

const MEDIA_FILE = /\/_media\/[0-9a-f]{64}\/([^/?#]+)$/;

/** The decoded name of a stored media file's URL (`/_media/<sha256>/<name>`), else null. */
export function mediaFileName(url: string): string | null {
  const name = MEDIA_FILE.exec(url)?.[1];
  if (name === undefined) return null;
  try {
    return decodeURIComponent(name);
  } catch {
    return name;
  }
}

/** Names of the media files one test uploaded. */
export class OwnMedia {
  private names = new Set<string>();

  /** Notes an upload by its `url` (the original's, as `POST /media` answers). */
  note(url: string): void {
    const name = mediaFileName(url);
    if (name !== null) this.names.add(name);
  }

  /** Whether `url` is one of the noted uploads or a variant of one. */
  isOwn(url: string): boolean {
    const name = mediaFileName(url);
    return name !== null && this.names.has(name);
  }

  clear(): void {
    this.names.clear();
  }
}

/**
 * The running test's uploads. A Playwright worker runs one test at a time, so one
 * instance per worker: the `guard` fixture clears it when a test starts, and both the
 * guard (uploads a page made) and `ContentApi.upload` (uploads through the API) note to it.
 */
export const ownMedia = new OwnMedia();
