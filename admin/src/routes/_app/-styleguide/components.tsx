// The shadcn components as restyled by the tokens, in their states (11 §7).

import {
  BoldIcon,
  CalendarClockIcon,
  ChevronDownIcon,
  CopyIcon,
  FileTextIcon,
  HomeIcon,
  ImageIcon,
  ItalicIcon,
  LayoutDashboardIcon,
  LinkIcon,
  LoaderCircleIcon,
  MoreHorizontalIcon,
  NewspaperIcon,
  PencilIcon,
  PlusIcon,
  SearchIcon,
  SettingsIcon,
  StrikethroughIcon,
  Trash2Icon,
  UsersIcon,
} from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { CONTENT_STATUSES } from "@/api/content";
import { KbdCombo } from "@/components/kit/kbd-combo";
import { RelativeTime } from "@/components/kit/relative-time";
import { SaveState, type SaveStateValue } from "@/components/kit/save-state";
import { StatusBadge } from "@/components/kit/status-badge";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Avatar, AvatarFallback, AvatarGroup } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import {
  Breadcrumb,
  BreadcrumbEllipsis,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import { Calendar } from "@/components/ui/calendar";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from "@/components/ui/command";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { Input } from "@/components/ui/input";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupInput,
  InputGroupText,
} from "@/components/ui/input-group";
import { Kbd } from "@/components/ui/kbd";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from "@/components/ui/sheet";
import {
  Sidebar,
  SidebarContent,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
} from "@/components/ui/sidebar";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { Toggle } from "@/components/ui/toggle";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Block, Section, Specimen, StateMatrix, stateClass, type StateName } from "./frame";

const ALL_STATES: StateName[] = ["default", "hover", "focus", "disabled", "loading", "error"];
const FIELD_STATES: StateName[] = ["default", "hover", "focus", "disabled", "error"];
const BUTTON_VARIANTS = [
  "default",
  "secondary",
  "outline",
  "ghost",
  "destructive",
  "link",
] as const;

export function ControlsSection() {
  const { t } = useTranslation(["styleguide", "common"]);
  const [published, setPublished] = useState(true);
  return (
    <Section id="controls">
      <Block title={t("controls.buttons.title")} description={t("controls.buttons.body")}>
        <StateMatrix
          states={ALL_STATES}
          rows={BUTTON_VARIANTS.map((variant) => ({
            label: variant,
            render: (state) =>
              state === "error" ? null : (
                <Button
                  variant={variant}
                  className={stateClass(state)}
                  disabled={state === "disabled" || state === "loading"}
                  aria-busy={state === "loading" || undefined}
                >
                  {state === "loading" ? (
                    <LoaderCircleIcon data-icon="inline-start" className="animate-spin" />
                  ) : null}
                  {state === "loading" ? t("controls.buttons.saving") : t("controls.buttons.label")}
                </Button>
              ),
          }))}
        />
      </Block>

      <Block title={t("controls.sizes.title")} description={t("controls.sizes.body")}>
        <Specimen className="flex flex-wrap items-center gap-3">
          <Button size="lg">{t("controls.sizes.lg")}</Button>
          <Button>{t("controls.sizes.default")}</Button>
          <Button size="sm" variant="outline">
            {t("controls.sizes.sm")}
          </Button>
          <Button size="xs" variant="outline">
            {t("controls.sizes.xs")}
          </Button>
          <Separator orientation="vertical" className="mx-1 h-6" />
          <Button size="icon" variant="outline" aria-label={t("controls.sizes.add")}>
            <PlusIcon />
          </Button>
          <Button size="icon-sm" variant="ghost" aria-label={t("controls.sizes.more")}>
            <MoreHorizontalIcon />
          </Button>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button>
                <PlusIcon data-icon="inline-start" />
                {t("controls.sizes.newPost")}
              </Button>
            </TooltipTrigger>
            <TooltipContent>
              <KbdCombo keys="c p" />
            </TooltipContent>
          </Tooltip>
        </Specimen>
      </Block>

      <Block title={t("controls.fields.title")} description={t("controls.fields.body")}>
        <StateMatrix
          states={FIELD_STATES}
          rows={[
            {
              label: "input",
              render: (s) => (
                <Input
                  aria-label={t("controls.fields.title")}
                  placeholder={t("controls.fields.placeholder")}
                  className={`w-32 ${stateClass(s) ?? ""}`}
                  disabled={s === "disabled"}
                  aria-invalid={s === "error" || undefined}
                />
              ),
            },
            {
              label: "input-group",
              render: (s) => (
                <InputGroup className={`w-36 ${stateClass(s) ?? ""}`}>
                  <InputGroupAddon>
                    <SearchIcon />
                  </InputGroupAddon>
                  <InputGroupInput
                    aria-label={t("controls.fields.search")}
                    placeholder={t("controls.fields.search")}
                    disabled={s === "disabled"}
                    aria-invalid={s === "error" || undefined}
                  />
                  <InputGroupAddon align="inline-end">
                    <Kbd>/</Kbd>
                  </InputGroupAddon>
                </InputGroup>
              ),
            },
            {
              label: "select",
              render: (s) => (
                <Select defaultValue="draft" disabled={s === "disabled"}>
                  <SelectTrigger
                    aria-label={t("controls.fields.status")}
                    className={`w-32 ${stateClass(s) ?? ""}`}
                    aria-invalid={s === "error" || undefined}
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {CONTENT_STATUSES.filter((x) => x !== "trash").map((x) => (
                      <SelectItem key={x} value={x}>
                        {t(`common:status.${x}`)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ),
            },
            {
              label: "checkbox",
              render: (s) => (
                <Checkbox
                  aria-label={t("controls.fields.check")}
                  defaultChecked={false}
                  className={stateClass(s)}
                  disabled={s === "disabled"}
                  aria-invalid={s === "error" || undefined}
                />
              ),
            },
            {
              label: "checked",
              render: (s) => (
                <Checkbox
                  aria-label={t("controls.fields.check")}
                  defaultChecked={true}
                  className={stateClass(s)}
                  disabled={s === "disabled"}
                  aria-invalid={s === "error" || undefined}
                />
              ),
            },
            {
              label: "mixed",
              render: (s) => (
                <Checkbox
                  aria-label={t("controls.fields.check")}
                  defaultChecked={"indeterminate"}
                  className={stateClass(s)}
                  disabled={s === "disabled"}
                  aria-invalid={s === "error" || undefined}
                />
              ),
            },
            {
              label: "switch",
              render: (s) =>
                s === "error" ? null : (
                  <Switch
                    aria-label={t("controls.fields.switch")}
                    defaultChecked={false}
                    className={stateClass(s)}
                    disabled={s === "disabled"}
                  />
                ),
            },
            {
              label: "switch on",
              render: (s) =>
                s === "error" ? null : (
                  <Switch
                    aria-label={t("controls.fields.switch")}
                    defaultChecked={true}
                    className={stateClass(s)}
                    disabled={s === "disabled"}
                  />
                ),
            },
            {
              label: "toggle",
              render: (s) =>
                s === "error" ? null : (
                  <Toggle
                    variant="outline"
                    aria-label={t("controls.toggles.bold")}
                    className={stateClass(s)}
                    disabled={s === "disabled"}
                    defaultPressed={s === "focus"}
                  >
                    <BoldIcon />
                  </Toggle>
                ),
            },
          ]}
        />
      </Block>

      <div className="grid gap-6 lg:grid-cols-[3fr_2fr]">
        <Block title={t("controls.form.title")} description={t("controls.form.body")}>
          <Specimen className="flex flex-col gap-5">
            <div className="flex flex-col gap-2">
              <Label htmlFor="sg-title">{t("controls.form.titleLabel")}</Label>
              <Input id="sg-title" defaultValue={t("controls.form.titleValue")} />
            </div>
            <div className="flex flex-col gap-2">
              <Label htmlFor="sg-slug">{t("controls.form.slug")}</Label>
              <InputGroup>
                <InputGroupAddon>
                  <InputGroupText className="font-mono">/blog/</InputGroupText>
                </InputGroupAddon>
                <InputGroupInput id="sg-slug" defaultValue="hello-world" className="font-mono" />
              </InputGroup>
              <p className="text-xs text-muted-foreground">{t("controls.form.slugHelp")}</p>
            </div>
            <div className="flex flex-col gap-2">
              <Label htmlFor="sg-excerpt">{t("controls.form.excerpt")}</Label>
              <Textarea id="sg-excerpt" rows={3} defaultValue={t("controls.form.excerptValue")} />
            </div>
            <div className="flex flex-col gap-2">
              <Label htmlFor="sg-slug-bad">{t("controls.form.invalid")}</Label>
              <Input
                id="sg-slug-bad"
                defaultValue="Hello World!"
                aria-invalid
                aria-describedby="sg-slug-bad-error"
              />
              <p id="sg-slug-bad-error" className="text-xs text-destructive">
                {t("controls.form.invalidHelp")}
              </p>
            </div>
            <Separator />
            <div className="flex items-center justify-between gap-4">
              <div className="flex flex-col gap-0.5">
                <Label htmlFor="sg-comments">{t("controls.form.comments")}</Label>
                <span className="text-sm text-muted-foreground">
                  {t("controls.form.commentsHelp")}
                </span>
              </div>
              <Switch id="sg-comments" checked={published} onCheckedChange={setPublished} />
            </div>
            <div className="flex items-center gap-2">
              <Checkbox id="sg-sticky" defaultChecked />
              <Label htmlFor="sg-sticky">{t("controls.form.sticky")}</Label>
            </div>
          </Specimen>
        </Block>
        <div className="flex flex-col gap-6">
          <Block title={t("controls.toggles.title")} description={t("controls.toggles.body")}>
            <Specimen className="flex flex-col gap-4">
              <ToggleGroup
                type="multiple"
                variant="outline"
                defaultValue={["bold"]}
                aria-label={t("controls.toggles.formatting")}
              >
                <ToggleGroupItem value="bold" aria-label={t("controls.toggles.bold")}>
                  <BoldIcon />
                </ToggleGroupItem>
                <ToggleGroupItem value="italic" aria-label={t("controls.toggles.italic")}>
                  <ItalicIcon />
                </ToggleGroupItem>
                <ToggleGroupItem value="strike" aria-label={t("controls.toggles.strike")}>
                  <StrikethroughIcon />
                </ToggleGroupItem>
                <ToggleGroupItem value="link" aria-label={t("controls.toggles.link")}>
                  <LinkIcon />
                </ToggleGroupItem>
              </ToggleGroup>
              <ToggleGroup
                type="single"
                defaultValue="list"
                aria-label={t("controls.toggles.view")}
                className="w-fit rounded-lg bg-muted p-0.5"
              >
                <ToggleGroupItem value="list">{t("controls.toggles.list")}</ToggleGroupItem>
                <ToggleGroupItem value="grid">{t("controls.toggles.grid")}</ToggleGroupItem>
              </ToggleGroup>
            </Specimen>
          </Block>
          <Block title={t("controls.tabs.title")} description={t("controls.tabs.body")}>
            <Specimen className="flex flex-col gap-5">
              <Tabs defaultValue="all">
                <TabsList>
                  <TabsTrigger value="all">{t("controls.tabs.all")}</TabsTrigger>
                  <TabsTrigger value="drafts">{t("controls.tabs.drafts")}</TabsTrigger>
                  <TabsTrigger value="scheduled">{t("controls.tabs.scheduled")}</TabsTrigger>
                </TabsList>
                <TabsContent value="all" className="pt-2 text-muted-foreground">
                  {t("controls.tabs.panel")}
                </TabsContent>
                <TabsContent value="drafts" className="pt-2 text-muted-foreground">
                  {t("controls.tabs.panel")}
                </TabsContent>
                <TabsContent value="scheduled" className="pt-2 text-muted-foreground">
                  {t("controls.tabs.panel")}
                </TabsContent>
              </Tabs>
              <Tabs defaultValue="content">
                <TabsList variant="line">
                  <TabsTrigger value="content">{t("controls.tabs.content")}</TabsTrigger>
                  <TabsTrigger value="seo">{t("controls.tabs.seo")}</TabsTrigger>
                  <TabsTrigger value="revisions">{t("controls.tabs.revisions")}</TabsTrigger>
                </TabsList>
                <TabsContent value="content" className="pt-2 text-muted-foreground">
                  {t("controls.tabs.panel")}
                </TabsContent>
                <TabsContent value="seo" className="pt-2 text-muted-foreground">
                  {t("controls.tabs.panel")}
                </TabsContent>
                <TabsContent value="revisions" className="pt-2 text-muted-foreground">
                  {t("controls.tabs.panel")}
                </TabsContent>
              </Tabs>
            </Specimen>
          </Block>
        </div>
      </div>
    </Section>
  );
}

const SAVE_STATES: SaveStateValue[] = ["saved", "saving", "dirty", "error", "offline"];
const minutesAgo = (m: number) => new Date(Date.now() - m * 60_000).toISOString();

export function DisplaySection() {
  const { t } = useTranslation("styleguide");
  return (
    <Section id="display">
      <div className="grid gap-6 lg:grid-cols-2">
        <Block title={t("display.status.title")} description={t("display.status.body")}>
          <Specimen className="flex flex-wrap gap-2">
            {CONTENT_STATUSES.map((s) => (
              <StatusBadge key={s} status={s} />
            ))}
          </Specimen>
        </Block>
        <Block title={t("display.badges.title")} description={t("display.badges.body")}>
          <Specimen className="flex flex-wrap items-center gap-2">
            <Badge>{t("display.badges.new")}</Badge>
            <Badge variant="secondary">{t("display.badges.count", { count: 12 })}</Badge>
            <Badge variant="outline">{t("display.badges.beta")}</Badge>
            <Badge variant="destructive">{t("display.badges.failing")}</Badge>
            <Badge variant="outline" className="font-mono">
              v1.4.2
            </Badge>
          </Specimen>
        </Block>
      </div>

      <div className="grid gap-6 lg:grid-cols-2">
        <Block title={t("display.save.title")} description={t("display.save.body")}>
          <Specimen className="flex flex-col divide-y py-1">
            {SAVE_STATES.map((s) => (
              <div key={s} className="flex items-center justify-between gap-4 py-2.5">
                <span className="font-mono text-xs text-muted-foreground">{s}</span>
                <SaveState
                  state={s}
                  {...(s === "saved" ? { at: new Date(2026, 9, 25, 14, 2) } : {})}
                />
              </div>
            ))}
          </Specimen>
        </Block>
        <Block title={t("display.time.title")} description={t("display.time.body")}>
          <Specimen className="flex flex-col divide-y py-1">
            {[2, 180, 60 * 24 * 5, 60 * 24 * 400].map((m) => (
              <div key={m} className="flex items-center justify-between gap-4 py-2.5">
                <span className="text-sm text-muted-foreground">{t("display.time.updated")}</span>
                <RelativeTime at={minutesAgo(m)} className="text-sm" />
              </div>
            ))}
          </Specimen>
        </Block>
      </div>

      <Block title={t("display.keys.title")} description={t("display.keys.body")}>
        <Specimen padded={false}>
          <ul className="grid divide-y sm:grid-cols-2 sm:divide-y-0">
            {(
              [
                ["mod+k", "palette"],
                ["?", "sheet"],
                ["g d", "dashboard"],
                ["g p", "posts"],
                ["c p", "newPost"],
                ["mod+s", "save"],
                ["mod+shift+p", "publish"],
                ["j", "down"],
              ] as const
            ).map(([keys, label]) => (
              <li
                key={keys}
                className="flex items-center justify-between gap-4 px-5 py-3 sm:border-b sm:odd:border-r sm:[&:nth-last-child(-n+2)]:border-b-0"
              >
                <span className="text-sm">{t(`display.keys.${label}`)}</span>
                <KbdCombo keys={keys} />
              </li>
            ))}
          </ul>
        </Specimen>
      </Block>

      <div className="grid gap-6 lg:grid-cols-3">
        <Block title={t("display.avatars.title")} description={t("display.avatars.body")}>
          <Specimen className="flex flex-col gap-4">
            <div className="flex items-center gap-3">
              <Avatar size="lg">
                <AvatarFallback>AL</AvatarFallback>
              </Avatar>
              <Avatar>
                <AvatarFallback>GH</AvatarFallback>
              </Avatar>
              <Avatar size="sm">
                <AvatarFallback>KM</AvatarFallback>
              </Avatar>
            </div>
            <AvatarGroup>
              {["A", "G", "K", "R"].map((i) => (
                <Avatar key={i}>
                  <AvatarFallback>{i}</AvatarFallback>
                </Avatar>
              ))}
            </AvatarGroup>
          </Specimen>
        </Block>
        <Block title={t("display.skeleton.title")} description={t("display.skeleton.body")}>
          <Specimen className="flex flex-col gap-3">
            <div className="flex items-center gap-3">
              <Skeleton className="size-9 rounded-full" />
              <div className="flex flex-1 flex-col gap-2">
                <Skeleton className="h-3.5 w-2/3" />
                <Skeleton className="h-3 w-1/3" />
              </div>
            </div>
            <Skeleton className="h-20 w-full rounded-lg" />
          </Specimen>
        </Block>
        <Block title={t("display.toasts.title")} description={t("display.toasts.body")}>
          <Specimen className="flex flex-wrap gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() => toast.success(t("display.toasts.saved"))}
            >
              {t("display.toasts.success")}
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() =>
                toast(t("display.toasts.trashed"), {
                  action: { label: t("display.toasts.undo"), onClick: () => undefined },
                })
              }
            >
              {t("display.toasts.withUndo")}
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => toast.warning(t("display.toasts.busy"))}
            >
              {t("display.toasts.warning")}
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => toast.error(t("display.toasts.failed"))}
            >
              {t("display.toasts.error")}
            </Button>
          </Specimen>
        </Block>
      </div>

      <Block title={t("display.card.title")} description={t("display.card.body")}>
        <div className="grid gap-4 md:grid-cols-2">
          <Card>
            <CardHeader>
              <CardTitle>{t("display.card.siteTitle")}</CardTitle>
              <CardDescription>{t("display.card.siteBody")}</CardDescription>
              <CardAction>
                <Badge variant="outline" className="text-success">
                  {t("display.card.healthy")}
                </Badge>
              </CardAction>
            </CardHeader>
            <CardContent className="grid grid-cols-3 gap-3 text-sm">
              {[
                ["display.card.nodes", "2"],
                ["display.card.jobs", "0"],
                ["display.card.cache", "94%"],
              ].map(([k, v]) => (
                <div key={k} className="flex flex-col">
                  <span className="text-muted-foreground">{t(k as "display.card.nodes")}</span>
                  <span className="text-lg font-semibold tabular-nums">{v}</span>
                </div>
              ))}
            </CardContent>
            <CardFooter className="justify-end">
              <Button variant="outline" size="sm">
                {t("display.card.details")}
              </Button>
            </CardFooter>
          </Card>
          <Card size="sm">
            <CardHeader>
              <CardTitle>{t("display.card.smallTitle")}</CardTitle>
              <CardDescription>{t("display.card.smallBody")}</CardDescription>
            </CardHeader>
            <CardContent>
              <ScrollArea className="h-28 rounded-md border">
                <ul className="divide-y text-sm">
                  {Array.from({ length: 8 }, (_, i) => (
                    <li key={i} className="flex items-center justify-between px-3 py-2">
                      <span>{t("display.card.job", { n: i + 1 })}</span>
                      <span className="font-mono text-xs text-muted-foreground tabular-nums">
                        {(i + 1) * 12} ms
                      </span>
                    </li>
                  ))}
                </ul>
              </ScrollArea>
            </CardContent>
          </Card>
        </div>
      </Block>
    </Section>
  );
}

const NAV = [
  { key: "dashboard", icon: LayoutDashboardIcon },
  { key: "posts", icon: NewspaperIcon, badge: 24 },
  { key: "pages", icon: FileTextIcon, badge: 6 },
  { key: "media", icon: ImageIcon },
  { key: "users", icon: UsersIcon },
  { key: "settings", icon: SettingsIcon },
] as const;

export function NavigationSection() {
  const { t } = useTranslation("styleguide");
  const [active, setActive] = useState<string>("posts");
  return (
    <Section id="navigation">
      <div className="grid gap-6 lg:grid-cols-[18rem_1fr]">
        <Block title={t("navigation.sidebar.title")} description={t("navigation.sidebar.body")}>
          <Specimen padded={false} className="overflow-hidden">
            <SidebarProvider className="min-h-0">
              <Sidebar collapsible="none" className="w-full">
                <SidebarHeader className="flex-row items-center gap-2 px-3 pt-3">
                  <span
                    aria-hidden="true"
                    className="flex size-7 items-center justify-center rounded-md bg-primary text-xs font-bold text-primary-foreground"
                  >
                    F
                  </span>
                  <span className="text-sm font-semibold">{t("navigation.sidebar.site")}</span>
                </SidebarHeader>
                <SidebarContent>
                  <SidebarGroup>
                    <SidebarGroupLabel>{t("navigation.sidebar.group")}</SidebarGroupLabel>
                    <SidebarGroupContent>
                      <SidebarMenu>
                        {NAV.map((item) => (
                          <SidebarMenuItem key={item.key}>
                            <SidebarMenuButton
                              isActive={active === item.key}
                              onClick={() => setActive(item.key)}
                            >
                              <item.icon />
                              <span>{t(`navigation.items.${item.key}`)}</span>
                            </SidebarMenuButton>
                            {"badge" in item ? (
                              <SidebarMenuBadge className="tabular-nums">
                                {item.badge}
                              </SidebarMenuBadge>
                            ) : null}
                          </SidebarMenuItem>
                        ))}
                      </SidebarMenu>
                    </SidebarGroupContent>
                  </SidebarGroup>
                </SidebarContent>
              </Sidebar>
            </SidebarProvider>
          </Specimen>
        </Block>
        <div className="flex min-w-0 flex-col gap-6">
          <Block
            title={t("navigation.breadcrumb.title")}
            description={t("navigation.breadcrumb.body")}
          >
            <Specimen>
              <Breadcrumb>
                <BreadcrumbList>
                  <BreadcrumbItem>
                    <BreadcrumbLink href="#navigation">
                      <HomeIcon className="size-3.5" aria-label={t("navigation.items.dashboard")} />
                    </BreadcrumbLink>
                  </BreadcrumbItem>
                  <BreadcrumbSeparator />
                  <BreadcrumbItem>
                    <BreadcrumbEllipsis />
                  </BreadcrumbItem>
                  <BreadcrumbSeparator />
                  <BreadcrumbItem>
                    <BreadcrumbLink href="#navigation">
                      {t("navigation.items.posts")}
                    </BreadcrumbLink>
                  </BreadcrumbItem>
                  <BreadcrumbSeparator />
                  <BreadcrumbItem>
                    <BreadcrumbPage>{t("controls.form.titleValue")}</BreadcrumbPage>
                  </BreadcrumbItem>
                </BreadcrumbList>
              </Breadcrumb>
            </Specimen>
          </Block>
          <Block title={t("navigation.command.title")} description={t("navigation.command.body")}>
            <Specimen padded={false} className="overflow-hidden">
              <Command label={t("navigation.command.title")} className="rounded-none!">
                <CommandInput placeholder={t("navigation.command.placeholder")} />
                <CommandList>
                  <CommandEmpty>{t("navigation.command.empty")}</CommandEmpty>
                  <CommandGroup heading={t("navigation.command.goTo")}>
                    <CommandItem>
                      <LayoutDashboardIcon />
                      {t("navigation.items.dashboard")}
                      <CommandShortcut>G D</CommandShortcut>
                    </CommandItem>
                    <CommandItem>
                      <NewspaperIcon />
                      {t("navigation.items.posts")}
                      <CommandShortcut>G P</CommandShortcut>
                    </CommandItem>
                  </CommandGroup>
                  <CommandGroup heading={t("navigation.command.create")}>
                    <CommandItem>
                      <PlusIcon />
                      {t("controls.sizes.newPost")}
                      <CommandShortcut>C P</CommandShortcut>
                    </CommandItem>
                    <CommandItem>
                      <CalendarClockIcon />
                      {t("navigation.command.schedule")}
                    </CommandItem>
                  </CommandGroup>
                </CommandList>
              </Command>
            </Specimen>
          </Block>
        </div>
      </div>
    </Section>
  );
}

export function OverlaysSection() {
  const { t } = useTranslation("styleguide");
  const [date, setDate] = useState<Date | undefined>(new Date(2026, 9, 28));
  return (
    <Section id="overlays">
      <Block title={t("overlays.triggers.title")} description={t("overlays.triggers.body")}>
        <Specimen className="flex flex-wrap items-center gap-2">
          <Dialog>
            <DialogTrigger asChild>
              <Button variant="outline">{t("overlays.dialog.open")}</Button>
            </DialogTrigger>
            <DialogContent>
              <DialogHeader>
                <DialogTitle>{t("overlays.dialog.title")}</DialogTitle>
                <DialogDescription>{t("overlays.dialog.body")}</DialogDescription>
              </DialogHeader>
              <div className="flex flex-col gap-2">
                <Label htmlFor="sg-dialog-when">{t("overlays.dialog.when")}</Label>
                <Input id="sg-dialog-when" defaultValue="2026-10-28 09:00" className="font-mono" />
              </div>
              <DialogFooter>
                <DialogClose asChild>
                  <Button variant="outline">{t("overlays.cancel")}</Button>
                </DialogClose>
                <Button>{t("overlays.dialog.confirm")}</Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>

          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button variant="destructive">
                <Trash2Icon data-icon="inline-start" />
                {t("overlays.alert.open")}
              </Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>{t("overlays.alert.title")}</AlertDialogTitle>
                <AlertDialogDescription>{t("overlays.alert.body")}</AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>{t("overlays.cancel")}</AlertDialogCancel>
                <AlertDialogAction variant="destructive">
                  {t("overlays.alert.confirm")}
                </AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>

          <Sheet>
            <SheetTrigger asChild>
              <Button variant="outline">{t("overlays.sheet.open")}</Button>
            </SheetTrigger>
            <SheetContent>
              <SheetHeader>
                <SheetTitle>{t("overlays.sheet.title")}</SheetTitle>
                <SheetDescription>{t("overlays.sheet.body")}</SheetDescription>
              </SheetHeader>
              <div className="flex flex-col gap-2 px-4">
                <Label htmlFor="sg-sheet-slug">{t("controls.form.slug")}</Label>
                <Input id="sg-sheet-slug" defaultValue="hello-world" className="font-mono" />
              </div>
              <SheetFooter>
                <Button>{t("overlays.sheet.done")}</Button>
              </SheetFooter>
            </SheetContent>
          </Sheet>

          <DropdownMenu modal={false}>
            <DropdownMenuTrigger asChild>
              <Button variant="outline">
                {t("overlays.menu.open")}
                <ChevronDownIcon data-icon="inline-end" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="w-56">
              <DropdownMenuLabel>{t("overlays.menu.label")}</DropdownMenuLabel>
              <DropdownMenuGroup>
                <DropdownMenuItem>
                  <PencilIcon />
                  {t("overlays.menu.edit")}
                  <DropdownMenuShortcut>E</DropdownMenuShortcut>
                </DropdownMenuItem>
                <DropdownMenuItem>
                  <CopyIcon />
                  {t("overlays.menu.duplicate")}
                </DropdownMenuItem>
                <DropdownMenuItem disabled>
                  <CalendarClockIcon />
                  {t("navigation.command.schedule")}
                </DropdownMenuItem>
              </DropdownMenuGroup>
              <DropdownMenuSeparator />
              <DropdownMenuItem variant="destructive">
                <Trash2Icon />
                {t("overlays.menu.trash")}
                <DropdownMenuShortcut>#</DropdownMenuShortcut>
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>

          <Popover>
            <PopoverTrigger asChild>
              <Button variant="outline">{t("overlays.popover.open")}</Button>
            </PopoverTrigger>
            <PopoverContent className="flex flex-col gap-2">
              <p className="font-medium">{t("overlays.popover.title")}</p>
              <p className="text-muted-foreground">{t("overlays.popover.body")}</p>
            </PopoverContent>
          </Popover>

          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost" size="icon" aria-label={t("overlays.tooltip.label")}>
                <SettingsIcon />
              </Button>
            </TooltipTrigger>
            <TooltipContent>
              {t("overlays.tooltip.label")} <Kbd>G S</Kbd>
            </TooltipContent>
          </Tooltip>

          <HoverCard>
            <HoverCardTrigger asChild>
              <Button variant="link">@ada</Button>
            </HoverCardTrigger>
            <HoverCardContent className="flex gap-3">
              <Avatar>
                <AvatarFallback>AL</AvatarFallback>
              </Avatar>
              <div className="flex flex-col gap-0.5">
                <span className="font-medium">Ada Lovelace</span>
                <span className="text-sm text-muted-foreground">{t("overlays.hover.body")}</span>
              </div>
            </HoverCardContent>
          </HoverCard>
        </Specimen>
      </Block>
      <Block title={t("overlays.calendar.title")} description={t("overlays.calendar.body")}>
        <Specimen className="w-fit p-2">
          <Calendar
            className="bg-card"
            mode="single"
            selected={date}
            onSelect={setDate}
            defaultMonth={new Date(2026, 9, 1)}
          />
        </Specimen>
      </Block>
    </Section>
  );
}
