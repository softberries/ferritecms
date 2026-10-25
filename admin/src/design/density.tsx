// Display density: comfortable (default) or compact (docs/design/11-admin-ui.md §4).

import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { applyDensity, storeDensity, storedDensity, type Density } from "./preferences";

export type { Density };

export interface DensityState {
  density: Density;
  setDensity: (density: Density) => void;
}

const DensityContext = createContext<DensityState | null>(null);

export function DensityProvider({ children }: { children: ReactNode }) {
  const [density, setDensityState] = useState<Density>(storedDensity);

  useEffect(() => applyDensity(density), [density]);

  const setDensity = useCallback((next: Density) => {
    storeDensity(next);
    setDensityState(next);
  }, []);

  const value = useMemo(() => ({ density, setDensity }), [density, setDensity]);
  return <DensityContext.Provider value={value}>{children}</DensityContext.Provider>;
}

export function useDensity(): DensityState {
  const state = useContext(DensityContext);
  if (!state) throw new Error("useDensity needs a <DensityProvider>");
  return state;
}
