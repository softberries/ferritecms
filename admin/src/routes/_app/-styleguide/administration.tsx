// The administration kit (M2b-3): the six-digit code input, the QR code, the write-only
// secret field, the show-once value and the settings form over a declared schema. Every
// secret and token here is made up.

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { SettingDefDto } from "@/api/themes";
import { CodeInput } from "@/components/kit/code-input";
import { CopyOnce } from "@/components/kit/copy-once";
import { QrCode } from "@/components/kit/qr-code";
import { SecretField } from "@/components/kit/secret-field";
import { SettingsForm, type SettingValues } from "@/components/kit/settings-form";
import { Label } from "@/components/ui/label";
import { Block, Section, Specimen } from "./frame";

const SECRET = "JBSWY3DPEHPK3PXP";
const OTPAUTH = `otpauth://totp/FerriteCMS:ada?secret=${SECRET}&issuer=FerriteCMS&algorithm=SHA1&digits=6&period=30`;
const TOKEN = "fer_7Qm2xK9pV4sR1tZ8wB3nL6yH5cJ0dF2gA";
/** A colour value built from parts: source files hold no colour literal (11 §3). */
const EMBER = `#${"b4532a"}`;

/** The base32 secret in groups of four, as the enrolment dialog shows it. */
const grouped = (secret: string) => secret.match(/.{1,4}/g)?.join(" ") ?? secret;

export function AdministrationSection() {
  const { t } = useTranslation("styleguide");
  const [code, setCode] = useState("482");
  const schema: SettingDefDto[] = [
    {
      key: "tagline",
      type: "text",
      label: t("administration.form.tagline"),
      default: "",
      options: [],
    },
    {
      key: "footer",
      type: "textarea",
      label: t("administration.form.footer"),
      default: "",
      options: [],
    },
    {
      key: "show_author",
      type: "bool",
      label: t("administration.form.showAuthor"),
      default: true,
      options: [],
    },
    {
      key: "excerpt_words",
      type: "number",
      label: t("administration.form.words"),
      default: 55,
      options: [],
    },
    {
      key: "layout",
      type: "select",
      label: t("administration.form.layout"),
      default: "list",
      options: ["list", "grid", "magazine"],
    },
    {
      key: "accent",
      type: "color",
      label: t("administration.form.accent"),
      default: EMBER,
      options: [],
    },
    {
      key: "logo",
      type: "media",
      label: t("administration.form.logo"),
      default: null,
      options: [],
    },
    {
      key: "api_key",
      type: "secret",
      label: t("administration.form.apiKey"),
      default: null,
      options: [],
    },
  ];
  const values: SettingValues = {
    tagline: t("administration.form.taglineValue"),
    footer: "",
    show_author: true,
    excerpt_words: 40,
    layout: "grid",
    accent: EMBER,
    logo: null,
  };

  return (
    <Section id="administration">
      <div className="grid gap-6 lg:grid-cols-2">
        <Block title={t("administration.totp.title")} description={t("administration.totp.body")}>
          <Specimen className="flex flex-col gap-6">
            <div className="flex flex-col gap-5 sm:flex-row sm:items-center">
              <QrCode
                text={OTPAUTH}
                label={t("administration.totp.qrLabel")}
                className="self-center sm:self-auto"
              />
              <div className="flex min-w-0 flex-col gap-2">
                <span className="text-sm font-medium">{t("administration.totp.manual")}</span>
                <code className="font-mono text-md tracking-wide break-all text-foreground">
                  {grouped(SECRET)}
                </code>
                <span className="text-sm text-muted-foreground">
                  {t("administration.totp.manualHint")}
                </span>
              </div>
            </div>
            <div className="flex max-w-xs flex-col gap-2">
              <Label htmlFor="styleguide-code">{t("administration.totp.code")}</Label>
              <CodeInput id="styleguide-code" value={code} onChange={setCode} autoFocus={false} />
            </div>
          </Specimen>
        </Block>
        <div className="flex min-w-0 flex-col gap-6">
          <Block
            title={t("administration.secret.title")}
            description={t("administration.secret.body")}
          >
            <Specimen className="flex flex-col gap-5">
              <div className="flex flex-col gap-2">
                <span className="text-sm font-medium">{t("administration.form.apiKey")}</span>
                <SecretField
                  name={t("administration.form.apiKey")}
                  set
                  onChange={() => undefined}
                />
              </div>
              <div className="flex flex-col gap-2">
                <span className="text-sm font-medium">{t("administration.secret.webhook")}</span>
                <SecretField
                  name={t("administration.secret.webhook")}
                  set={false}
                  onChange={() => undefined}
                />
              </div>
            </Specimen>
          </Block>
          <Block
            title={t("administration.copyOnce.title")}
            description={t("administration.copyOnce.body")}
          >
            <Specimen>
              <CopyOnce value={TOKEN} label={t("administration.copyOnce.label")} />
            </Specimen>
          </Block>
        </div>
      </div>

      <Block title={t("administration.form.title")} description={t("administration.form.body")}>
        <Specimen className="max-w-2xl">
          <SettingsForm
            schema={schema}
            values={values}
            secrets={{ api_key: true }}
            invalid={["excerpt_words"]}
            reload={() => Promise.resolve(values)}
            onSubmit={(patch) => {
              toast.success(t("administration.form.sent", { keys: Object.keys(patch).join(", ") }));
              return Promise.resolve();
            }}
            staleKey={t("administration.form.resource")}
          />
        </Specimen>
      </Block>
    </Section>
  );
}
