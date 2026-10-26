// The URL section: the slug, edited in place, and the address it gives. While the slug
// differs from the stored one the address shown is the predicted one, marked "after you
// save"; once saved it is the server's `permalink` (the server may normalise the slug or
// add `-2`). The copy button copies the stored address. An emptied slug puts the
// stored one back (the API refuses an empty slug).

import { CheckIcon, CopyIcon } from "lucide-react";
import { useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { EditorDocument } from "../editor-state";
import { predictPermalink } from "./publishing";

export interface SlugFieldProps {
  doc: EditorDocument;
  siteUrl: string | null;
  disabled: boolean;
}

/** Characters the server refuses in a slug (`slug::validate`), for the hint. */
const FORBIDDEN = /[\s/?#%<>"'\\`]/;

export function SlugField({ doc, siteUrl, disabled }: SlugFieldProps) {
  const { t } = useTranslation("editor");
  const id = useId();
  const hintId = useId();
  const { item } = doc;
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1_500);
    return () => window.clearTimeout(timer);
  }, [copied]);

  const draft = doc.meta.slug;
  const changed = draft.trim() !== "" && draft.trim().toLowerCase() !== doc.savedMeta.slug;
  const path = item ? predictPermalink(item.permalink, doc.savedMeta.slug, draft) : null;
  const host = siteUrl ? siteUrl.replace(/^https?:\/\//, "").replace(/\/$/, "") : "";
  const absolute = item && siteUrl ? new URL(item.permalink, siteUrl).toString() : null;
  const bad = FORBIDDEN.test(draft.trim());

  const copy = async () => {
    if (!absolute) return;
    try {
      await navigator.clipboard.writeText(absolute);
      setCopied(true);
      toast.success(t("slug.copied"), { id: "editor-copy" });
    } catch {
      toast.error(t("slug.copyFailed"), { id: "editor-copy" });
    }
  };

  return (
    <div className="flex flex-col gap-1.5">
      <Label htmlFor={id}>{t("slug.label")}</Label>
      <div className="flex items-center gap-1.5">
        <Input
          id={id}
          data-slot="slug-input"
          value={draft}
          disabled={disabled}
          placeholder={item ? undefined : t("slug.placeholderNew")}
          spellCheck={false}
          autoCapitalize="off"
          autoComplete="off"
          aria-describedby={hintId}
          aria-invalid={bad ? true : undefined}
          onChange={(e) => doc.setMeta({ slug: e.target.value })}
          onBlur={() => {
            if (draft.trim() === "" && doc.savedMeta.slug !== "") {
              doc.setMeta({ slug: doc.savedMeta.slug });
            }
          }}
          className="h-8 min-w-0 flex-1 font-mono text-[0.8125rem]"
        />
        {absolute ? (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                aria-label={t("slug.copy")}
                onClick={() => {
                  void copy();
                }}
                className="text-muted-foreground"
              >
                {copied ? <CheckIcon /> : <CopyIcon />}
              </Button>
            </TooltipTrigger>
            <TooltipContent>{t("slug.copy")}</TooltipContent>
          </Tooltip>
        ) : null}
      </div>
      <p
        id={hintId}
        data-slot="permalink"
        className="text-xs leading-5 break-all text-muted-foreground"
      >
        {bad ? (
          <span className="text-destructive">{t("slug.invalid")}</span>
        ) : item ? (
          path ? (
            <>
              <span className="font-mono">
                {host}
                <span className="text-foreground">{path}</span>
              </span>
              {changed ? (
                <span className="ml-1.5 whitespace-nowrap">{t("slug.afterSave")}</span>
              ) : null}
            </>
          ) : (
            t("slug.unknownUntilSave")
          )
        ) : (
          t("slug.newHint")
        )}
      </p>
    </div>
  );
}
