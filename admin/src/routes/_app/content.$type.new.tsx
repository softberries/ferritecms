// /admin/content/$type/new: a new item in the writing surface. Nothing is stored until
// the first save (`mod+s` or Save): that `POST`s the draft and replaces the address with
// the item's `/$id`, handing the live editor state over so writing continues where it
// was (the two routes share one route transition, so nothing flashes).
//
// A new item has no autosave slot (no id yet): the leave guard protects its draft until
// the first save. A `401` on that save keeps the draft (`pendingDrafts`, key
// `new:<type>`); after the sign-in the screen opens with it again (Task 16).

import {
  createFileRoute,
  notFound,
  useLocation,
  useNavigate,
  useRouter,
} from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { typesQuery } from "@/api/types";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { dropPendingDraft, newDraftKey, peekPendingDraft } from "@/editor/autosave";
import { EditorScreen } from "@/editor/editor";
import type { RestoreSource } from "@/editor/editor-state";
import { handOff } from "@/editor/handoff";

/** Types edited elsewhere (the media library, M2b-2). */
const NOT_EDITED = new Set(["media"]);

interface NewLoaderData {
  label: string;
}

export const Route = createFileRoute("/_app/content/$type/new")({
  staticData: { crumb: (t) => t("crumbs.new"), transitionKey: "editor" },
  loader: async ({ context, params }): Promise<NewLoaderData> => {
    const types = await context.queryClient.ensureQueryData(typesQuery);
    const type = types.find((ty) => ty.name === params.type);
    if (!type || NOT_EDITED.has(type.name)) throw notFound();
    return { label: type.label };
  },
  component: NewItem,
});

function NewItem() {
  const { type } = Route.useParams();
  const { label } = Route.useLoaderData();
  const { me: guarded } = Route.useRouteContext();
  const me = useMe() ?? guarded;
  const navigate = useNavigate();
  const router = useRouter();
  const newPath = useLocation({ select: (l) => l.pathname });
  // A draft kept over a sign-in (read once; Discard starts an empty screen).
  const [pending] = useState(() => peekPendingDraft(newDraftKey(type), me.id));
  const [start, setStart] = useState<{ epoch: number; restore: RestoreSource | null }>(() => ({
    epoch: 0,
    restore: pending ? { draft: pending.draft, at: pending.at } : null,
  }));
  // Taken: the screen holds it now (and the leave guard protects it).
  useEffect(() => {
    if (pending) dropPendingDraft(newDraftKey(type), pending);
  }, [pending, type]);

  // The SPA hides what the server would refuse (it never decides): no `edit_own`, no
  // writing surface.
  if (!me.capabilities.includes("edit_own")) return <ForbiddenState className="mt-12" />;

  return (
    <EditorScreen
      key={start.epoch}
      type={type}
      typeLabel={label}
      item={null}
      restore={start.restore}
      onReopen={() => setStart((s) => ({ epoch: s.epoch + 1, restore: null }))}
      onCreated={(item, handoff) => {
        // Moved on while the first save was in flight: the draft exists, stay where they are.
        if (router.state.location.pathname !== newPath) return;
        handOff(handoff);
        void navigate({
          to: "/content/$type/$id",
          params: { type, id: item.id },
          replace: true,
          ignoreBlocker: true,
        });
      }}
    />
  );
}
