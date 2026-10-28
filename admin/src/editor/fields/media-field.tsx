// A `media` field: `MediaChoice` (the kit's media picker control) over any kind, since the
// schema does not say which.

import { useTranslation } from "react-i18next";
import { MediaChoice } from "@/components/kit/media-choice";
import type { ControlIds } from "./fields-context";

export function MediaField({
  ids,
  value,
  onChange,
  disabled,
}: {
  ids: ControlIds;
  value: unknown;
  onChange: (value: unknown) => void;
  disabled: boolean;
}) {
  const { t } = useTranslation("editor");
  return (
    <MediaChoice
      value={typeof value === "string" ? value : null}
      onChange={onChange}
      pickTitle={t("fields.pickFile")}
      chooseLabel={t("fields.chooseFile")}
      disabled={disabled}
      ids={ids}
    />
  );
}
