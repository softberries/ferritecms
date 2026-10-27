// React 19 reports every error an error boundary caught as a `console.error`. A route's
// error component shows an API problem (403, 404) or a failed load (no response at all,
// `NetworkError`; Firefox and WebKit also fail a page's pending requests this way while
// it navigates away) as a state of the screen with a retry, so those are handled answers
// and are not reported; anything else is.

import type { ErrorInfo } from "react";
import { NetworkError } from "@/api/client";
import { ApiError } from "@/api/problem";

export function reportCaughtError(error: unknown, info: ErrorInfo): void {
  if (error instanceof ApiError || error instanceof NetworkError) return;
  console.error(error, info.componentStack);
}
