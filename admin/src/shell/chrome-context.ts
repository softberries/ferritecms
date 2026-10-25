// What a screen may ask of the shell around it. The editor's focus mode (`mod+shift+f`)
// hides the sidebar and the top bar; the shell owns them, so the screen asks here
// instead of reaching into the DOM. Outside the shell (tests, bare routes) the default
// does nothing.

import { createContext, useContext } from "react";

export interface ShellChrome {
  /** The shell's sidebar and top bar are hidden (the editor's focus mode). */
  focusMode: boolean;
  setFocusMode: (on: boolean) => void;
}

export const ShellChromeContext = createContext<ShellChrome>({
  focusMode: false,
  setFocusMode: () => {},
});

export function useShellChrome(): ShellChrome {
  return useContext(ShellChromeContext);
}
