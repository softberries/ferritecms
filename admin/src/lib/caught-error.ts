// React 19 reports every error an error boundary caught as a `console.error`. A route's
// error component shows an API problem (403, 404, a failed load) as a state of the
// screen, so an `ApiError` is a handled answer and is not reported; anything else is.

import type { ErrorInfo } from "react";
import { ApiError } from "@/api/problem";

export function reportCaughtError(error: unknown, info: ErrorInfo): void {
  if (error instanceof ApiError) return;
  console.error(error, info.componentStack);
}
