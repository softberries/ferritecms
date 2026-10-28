// WebKit's reports of a load that a navigation cancelled (10 §3), recognised in the shapes
// Playwright 1.64 passes them on in. Used by the guard in `fixtures.ts`.

const FETCH_CANCELLED = /^Fetch API cannot load (\S+) due to access control checks\.$/;
const IMPORT_CANCELLED = "TypeError: Importing a module script failed.";

/** A cancelled `fetch` of `url`, or a cancelled lazy module (`import`). */
export type CancelledLoad = { kind: "fetch"; url: string } | { kind: "import" };

/** The cancelled load a console line reports, if it reports one. */
export function cancelledInConsole(text: string): CancelledLoad | null {
  return classify(text.split("\n")[0] ?? "");
}

/**
 * The cancelled load a page error reports, if it reports one.
 *
 * Playwright builds the error from WebKit's text: `stack` is that text plus the frames,
 * or empty when WebKit sent no frames, and `name` and `message` are the text split at its
 * first ":" with the character after it dropped (`splitErrorMessage`). So "Fetch API
 * cannot load http://host/x due to …" arrives without a stack as name "Fetch API cannot
 * load http" and message "/host/x due to …", and the text is rebuilt from both.
 */
export function cancelledInPageError(e: {
  name: string;
  message: string;
  stack?: string | undefined;
}): CancelledLoad | null {
  const candidates = [`${e.name}: ${e.message}`, `${e.name}:/${e.message}`];
  if (e.stack) candidates.unshift(e.stack.split("\n")[0] ?? "");
  for (const text of candidates) {
    const found = classify(text);
    if (found) return found;
  }
  return null;
}

function classify(text: string): CancelledLoad | null {
  if (text === IMPORT_CANCELLED) return { kind: "import" };
  const url = FETCH_CANCELLED.exec(text)?.[1];
  return url ? { kind: "fetch", url } : null;
}
