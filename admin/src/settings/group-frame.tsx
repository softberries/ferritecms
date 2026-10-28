// The frame of a settings group: its heading and description, then a card. An editable
// group's card is a form with one primary action, Save, in its footer.

import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Alert } from "@/auth/auth-layout";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardFooter } from "@/components/ui/card";
import type { FrameForm } from "./use-group-form";

export interface GroupFrameProps {
  title: string;
  description: string;
  children: ReactNode;
}

/** The group's heading (the page's `h2`) and description. */
export function GroupHeading({ title, description }: { title: string; description: string }) {
  return (
    <div className="flex flex-col gap-1">
      <h2 id="settings-group-title" className="text-lg text-foreground">
        {title}
      </h2>
      <p className="max-w-prose text-sm text-muted-foreground">{description}</p>
    </div>
  );
}

/** A read-only group: heading, then its content. */
export function GroupFrame({ title, description, children }: GroupFrameProps) {
  return (
    <div className="flex min-w-0 flex-col gap-5">
      <GroupHeading title={title} description={description} />
      {children}
    </div>
  );
}

/** An editable group: the form's fields in a card, Save in its footer. */
export function GroupFormFrame({
  title,
  description,
  form,
  children,
}: GroupFrameProps & { form: FrameForm }) {
  const { t } = useTranslation("admin");
  return (
    <div className="flex min-w-0 flex-col gap-5">
      <GroupHeading title={title} description={description} />
      <Card>
        <form
          id={form.formId}
          noValidate
          onSubmit={(e) => {
            e.preventDefault();
            void form.save();
          }}
          className="flex flex-col gap-(--card-spacing)"
        >
          <CardContent className="flex flex-col gap-6">
            {form.failure ? <Alert tone="error">{form.failure}</Alert> : null}
            {children}
          </CardContent>
          <CardFooter className="flex-wrap justify-end gap-3 border-t py-3">
            {form.dirty && !form.saving ? (
              <span
                data-slot="settings-pending"
                className="mr-auto text-sm text-muted-foreground tabular-nums"
              >
                {t("settings.pending", { count: form.changed.length })}
              </span>
            ) : null}
            <Button type="submit" disabled={!form.dirty || form.saving} aria-busy={form.saving}>
              {form.saving ? t("settings.saving") : t("settings.save")}
            </Button>
          </CardFooter>
        </form>
      </Card>
      {form.dialogs}
    </div>
  );
}
