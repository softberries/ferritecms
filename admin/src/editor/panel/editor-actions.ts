// The editor screen's release dialogs as the panel's buttons reach them (M6-3): Add to
// release…, Save as change… and Share preview… live on the screen (they outlast the phone's
// settings sheet), so the status section and the publish bar ask for them here. Outside an
// editor screen (tests) they do nothing.

import { createContext, useContext } from "react";

export interface EditorActions {
  /** Opens Add to release (`asChange`: Save as change, the draft always staged). */
  release: (asChange: boolean) => void;
  share: () => void;
}

export const EditorActionsContext = createContext<EditorActions>({
  release: () => {},
  share: () => {},
});

export function useEditorActions(): EditorActions {
  return useContext(EditorActionsContext);
}
