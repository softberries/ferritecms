import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import auth from "./en/auth.json";
import common from "./en/common.json";
import content from "./en/content.json";
import dashboard from "./en/dashboard.json";
import palette from "./en/palette.json";
import shell from "./en/shell.json";
import styleguide from "./en/styleguide.json";

/** Every namespace, one per area of the admin. English only in M2b. */
export const resources = {
  en: { common, auth, shell, styleguide, palette, dashboard, content },
} as const;

export const defaultNS = "common";

declare module "i18next" {
  interface CustomTypeOptions {
    defaultNS: typeof defaultNS;
    resources: (typeof resources)["en"];
    returnNull: false;
  }
}

export interface InitI18nOptions {
  /** Called for every key `t()` cannot find (tests turn this into a failure). */
  onMissingKey?: (ns: string, key: string) => void;
}

/** Initialises the shared i18next instance; resolves once it can translate. */
export async function initI18n(options: InitI18nOptions = {}): Promise<void> {
  const { onMissingKey } = options;
  await i18n.use(initReactI18next).init({
    resources,
    lng: "en",
    fallbackLng: "en",
    ns: Object.keys(resources.en),
    defaultNS,
    returnNull: false,
    interpolation: { escapeValue: false },
    saveMissing: onMissingKey !== undefined,
    missingKeyHandler: onMissingKey
      ? (_lngs, ns, key) => {
          onMissingKey(ns, key);
        }
      : false,
  });
}

export { i18n };
