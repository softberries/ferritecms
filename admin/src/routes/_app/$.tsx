import { createFileRoute } from "@tanstack/react-router";
import { NotFound } from "@/shell/not-found";

// Every admin URL no route claims: the not-found screen inside the shell, behind the
// guard (so an unauthenticated visitor learns nothing about which screens exist). Not a
// `/not-found` route (pre-flight F20): the address stays as typed.
export const Route = createFileRoute("/_app/$")({
  staticData: { crumb: (t) => t("crumbs.notFound") },
  component: NotFound,
});
