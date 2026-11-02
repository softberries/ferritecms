// AI assistants (12 §2, §5.4, §8): the `mcp.enabled` switch (off: `/mcp`, the OAuth
// endpoints and their metadata answer 404; connected apps pause and work again when it
// is back on), the MCP URL to paste, how to connect Claude, ChatGPT, Claude Code, Cursor
// and VS Code (exact steps where they are stable, the client's connector settings where
// menu names move), what an assistant can do, and what stays with people.

import { useQuery } from "@tanstack/react-query";
import {
  ArrowUpRightIcon,
  CheckIcon,
  CircleUserRoundIcon,
  HandIcon,
  ScrollTextIcon,
  ShieldCheckIcon,
  TriangleAlertIcon,
} from "lucide-react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { metaQuery } from "@/api/meta";
import { mcpUrl } from "@/api/oauth";
import type { SettingsDto } from "@/api/settings";
import { AI_CLIENTS, AI_GUIDE_URL, clientSnippets } from "@/oauth/model";
import { CopyCode, CopyField } from "@/components/kit/copy-field";
import { SwitchRow } from "@/components/kit/form-field";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { boolSpec } from "./fields";
import { GroupFormFrame } from "./group-frame";
import { useGroupForm } from "./use-group-form";

/** `MCP_ENABLED_DEFAULT` in `ferrite-core/src/settings.rs`. */
const MCP_ENABLED_DEFAULT = true;

const SPECS = { "mcp.enabled": boolSpec(MCP_ENABLED_DEFAULT) };

export function AiAssistantsGroup({ settings }: { settings: SettingsDto }) {
  const { t } = useTranslation("admin");
  const { t: ta } = useTranslation("ai");
  const form = useGroupForm(SPECS, settings, {
    resource: t("settings.resource", { group: t("settings.groups.aiAssistants.title") }),
  });
  const site = useQuery(metaQuery);
  const url = site.data ? mcpUrl(site.data.site_url) : null;
  const on = (settings["mcp.enabled"] ?? MCP_ENABLED_DEFAULT) === true;

  return (
    <div className="flex min-w-0 flex-col gap-8">
      <GroupFormFrame
        title={t("settings.groups.aiAssistants.title")}
        description={t("settings.groups.aiAssistants.description")}
        form={form}
      >
        <SwitchRow label={ta("settings.enabled")} hint={ta("settings.enabledHint")}>
          {(control) => (
            <Switch
              {...control}
              data-slot="mcp-enabled"
              checked={form.draft["mcp.enabled"]}
              disabled={form.saving}
              onCheckedChange={(v) => form.set("mcp.enabled", v)}
            />
          )}
        </SwitchRow>
        <ul className="grid gap-3 border-t pt-5 sm:grid-cols-3">
          <Safety icon={CircleUserRoundIcon}>{ta("settings.safety.login.body")}</Safety>
          <Safety icon={ShieldCheckIcon}>{ta("settings.safety.role.body")}</Safety>
          <Safety icon={ScrollTextIcon}>{ta("settings.safety.audited.body")}</Safety>
        </ul>
      </GroupFormFrame>

      <ConnectSection url={url} on={on} />
      <CanSection />
    </div>
  );
}

function Safety({ icon: Icon, children }: { icon: typeof ShieldCheckIcon; children: ReactNode }) {
  return (
    <li className="flex items-start gap-2.5 text-sm text-muted-foreground">
      <Icon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-primary" />
      <span className="text-pretty">{children}</span>
    </li>
  );
}

function SectionHeading({
  id,
  title,
  description,
}: {
  id: string;
  title: string;
  description: string;
}) {
  return (
    <div className="flex flex-col gap-1">
      <h3 id={id} className="text-md font-semibold text-foreground">
        {title}
      </h3>
      <p className="max-w-prose text-sm text-muted-foreground">{description}</p>
    </div>
  );
}

function ConnectSection({ url, on }: { url: string | null; on: boolean }) {
  const { t } = useTranslation("ai");
  return (
    <section
      aria-labelledby="ai-connect-title"
      data-slot="ai-connect"
      className="flex min-w-0 flex-col gap-4"
    >
      <SectionHeading
        id="ai-connect-title"
        title={t("settings.connect.title")}
        description={t("settings.connect.description")}
      />
      <Card>
        <CardContent className="flex min-w-0 flex-col gap-6">
          {on ? null : (
            <p
              role="status"
              className="flex items-start gap-2.5 rounded-lg border border-warning/50 bg-warning/8 px-3 py-2.5 text-sm text-foreground"
            >
              <TriangleAlertIcon
                aria-hidden="true"
                className="mt-0.5 size-4 shrink-0 text-warning"
              />
              {t("settings.connect.off")}
            </p>
          )}
          {url ? (
            <CopyField value={url} label={t("settings.connect.url")} />
          ) : (
            <div aria-busy="true" className="flex flex-col gap-2">
              <Skeleton className="h-4 w-20" />
              <Skeleton className="h-control w-full" />
            </div>
          )}
          {url ? <ClientTabs url={url} /> : null}
          <a
            href={AI_GUIDE_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex min-h-11 items-center gap-1.5 self-start rounded-sm text-sm font-medium text-primary underline-offset-4 hover:underline"
          >
            {t("settings.connect.guide")}
            <ArrowUpRightIcon aria-hidden="true" className="size-4" />
          </a>
        </CardContent>
      </Card>
    </section>
  );
}

function ClientTabs({ url }: { url: string }) {
  const { t } = useTranslation("ai");
  const snippets = clientSnippets(url);
  return (
    <Tabs defaultValue="claude" className="min-w-0 gap-4" data-slot="ai-clients">
      <div className="-mx-1 min-w-0 overflow-x-auto px-1 pb-1">
        <TabsList aria-label={t("settings.connect.clients")} className="w-max">
          {AI_CLIENTS.map((client) => (
            <TabsTrigger key={client} value={client} className="px-3 pointer-coarse:min-h-11">
              {t(`settings.clients.${client}.tab`)}
            </TabsTrigger>
          ))}
        </TabsList>
      </div>
      <TabsContent value="claude" className="flex flex-col gap-3">
        <Steps
          lead={t("settings.clients.claude.lead")}
          steps={[
            t("settings.clients.claude.step1"),
            t("settings.clients.claude.step2"),
            t("settings.clients.claude.step3"),
          ]}
        />
      </TabsContent>
      <TabsContent value="chatgpt" className="flex flex-col gap-3">
        <Steps
          lead={t("settings.clients.chatgpt.lead")}
          steps={[
            t("settings.clients.chatgpt.step1"),
            t("settings.clients.chatgpt.step2"),
            t("settings.clients.chatgpt.step3"),
          ]}
        />
      </TabsContent>
      <TabsContent value="claudeCode" className="flex min-w-0 flex-col gap-3">
        <p className="text-muted-foreground">{t("settings.clients.claudeCode.lead")}</p>
        <CopyCode value={snippets.claudeCode} label={t("settings.clients.claudeCode.command")} />
        <p className="text-muted-foreground">{t("settings.clients.claudeCode.then")}</p>
        <p className="pt-2 text-muted-foreground">{t("settings.clients.claudeCode.token")}</p>
        <CopyCode
          value={snippets.claudeCodeToken}
          label={t("settings.clients.claudeCode.tokenCommand")}
        />
      </TabsContent>
      <TabsContent value="cursor" className="flex min-w-0 flex-col gap-3">
        <p className="text-muted-foreground">{t("settings.clients.cursor.lead")}</p>
        <CopyCode value={snippets.cursor} label="~/.cursor/mcp.json" />
        <p className="text-muted-foreground">{t("settings.clients.cursor.then")}</p>
      </TabsContent>
      <TabsContent value="vscode" className="flex min-w-0 flex-col gap-3">
        <p className="text-muted-foreground">{t("settings.clients.vscode.lead")}</p>
        <CopyCode value={snippets.vscode} label=".vscode/mcp.json" />
        <p className="text-muted-foreground">{t("settings.clients.vscode.then")}</p>
      </TabsContent>
    </Tabs>
  );
}

function Steps({ lead, steps }: { lead: string; steps: string[] }) {
  return (
    <>
      <p className="text-muted-foreground">{lead}</p>
      <ol className="flex flex-col gap-2.5">
        {steps.map((step, i) => (
          <li key={i} className="flex items-start gap-3 text-foreground">
            <span
              aria-hidden="true"
              className="flex size-6 shrink-0 items-center justify-center rounded-full border bg-muted text-xs font-semibold tabular-nums"
            >
              {i + 1}
            </span>
            <span className="pt-0.5 text-pretty">{step}</span>
          </li>
        ))}
      </ol>
    </>
  );
}

function CanSection() {
  const { t } = useTranslation("ai");
  const can = ["content", "media", "structure", "comments", "site", "system"] as const;
  return (
    <section aria-labelledby="ai-can-title" className="flex min-w-0 flex-col gap-4">
      <SectionHeading
        id="ai-can-title"
        title={t("settings.can.title")}
        description={t("settings.can.description")}
      />
      <Card>
        <CardContent className="grid gap-6 md:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
          <ul className="flex flex-col gap-2.5">
            {can.map((key) => (
              <li key={key} className="flex items-start gap-2.5 text-sm text-foreground">
                <CheckIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-success" />
                {t(`settings.can.${key}`)}
              </li>
            ))}
          </ul>
          <div className="flex flex-col gap-2 rounded-lg border bg-muted/50 p-4 text-sm">
            <span className="flex items-center gap-2 font-medium text-foreground">
              <HandIcon aria-hidden="true" className="size-4 shrink-0 text-primary" />
              {t("settings.human.title")}
            </span>
            <p className="text-pretty text-muted-foreground">{t("settings.human.body")}</p>
          </div>
        </CardContent>
      </Card>
    </section>
  );
}
