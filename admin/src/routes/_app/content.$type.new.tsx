// /admin/content/$type/new: a new item in the writing surface. Nothing is stored until
// the first save (`mod+s` or Save): that `POST`s the draft and replaces the address with
// the item's `/$id`, handing the live editor state over so writing continues where it
// was (the two routes share one route transition, so nothing flashes).

import {
  createFileRoute,
  notFound,
  useLocation,
  useNavigate,
  useRouter,
} from "@tanstack/react-router";
import { typesQuery } from "@/api/types";
import { useMe } from "@/auth/session";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { EditorScreen } from "@/editor/editor";
import { handOff } from "@/editor/editor-state";

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

  // The SPA hides what the server would refuse (it never decides): no `edit_own`, no
  // writing surface.
  if (!me.capabilities.includes("edit_own")) return <ForbiddenState className="mt-12" />;

  return (
    <EditorScreen
      type={type}
      typeLabel={label}
      item={null}
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
