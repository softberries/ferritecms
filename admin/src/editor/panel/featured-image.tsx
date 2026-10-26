// The featured image (07 §3 Editor; a panel section for every type): empty, "Set featured
// image" opens the media picker for images; set, the image with Replace and Remove. It is
// stored as `fields.featured_image` (a UUID or `null`; core allows it in every schema,
// F17), so it saves, autosaves and conflicts with the other fields.

import { useTranslation } from "react-i18next";
import type { EditorDocument } from "../editor-state";
import { MediaChoice } from "../fields/media-field";
import { FEATURED_IMAGE } from "../fields/fields-model";

export function FeaturedImage({ doc, disabled }: { doc: EditorDocument; disabled: boolean }) {
  const { t } = useTranslation("editor");
  const raw = doc.meta.fields[FEATURED_IMAGE];
  const value = typeof raw === "string" ? raw : null;
  return (
    <section
      aria-labelledby="panel-featured"
      data-slot="panel-featured"
      className="flex flex-col gap-2"
    >
      <h3 id="panel-featured" className="text-sm font-medium">
        {t("featured.heading")}
      </h3>
      <MediaChoice
        large
        kind="image"
        value={value}
        onChange={(id) => doc.setField(FEATURED_IMAGE, id)}
        pickTitle={t("featured.pickTitle")}
        chooseLabel={t("featured.set")}
        disabled={disabled}
      />
    </section>
  );
}
