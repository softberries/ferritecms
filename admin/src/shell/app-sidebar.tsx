// The shell's sidebar: the site mark, the nav sections from `navFor`, and a link to the
// public site. Desktop: collapsible to an icon rail (`mod+b`, state per viewer); under
// 768 px the generated sidebar renders it in a `Sheet` instead.

import { Link, useLocation } from "@tanstack/react-router";
import { ExternalLinkIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSkeleton,
  SidebarRail,
  useSidebar,
} from "@/components/ui/sidebar";
import type { NavItem, NavSection } from "./nav";

export interface AppSidebarProps {
  sections: NavSection[];
  /** The content types are still loading: the Content section shows skeleton rows. */
  loading: boolean;
  siteTitle: string;
  siteUrl: string | null;
  /** Slid fully out (the editor's focus mode) instead of collapsing to the icon rail. */
  hidden?: boolean;
}

/** The path inside the admin (`/`, `/content/post`), whatever the router reports. */
function adminPathname(pathname: string): string {
  const inside = pathname.replace(/^\/admin(?=\/|$)/, "");
  return inside === "" ? "/" : inside;
}

function isActive(item: NavItem, pathname: string): boolean {
  if (item.to === "/") return pathname === "/";
  return pathname === item.to || pathname.startsWith(`${item.to}/`);
}

// Rows are control height: 36 px comfortable, 30 px compact, 44 px on touch screens
// (`--control-h`, 11 §4, §8); the active one is tinted with its icon in the accent,
// the rest stay quiet.
const ROW =
  "h-control gap-2.5 px-2.5 text-sidebar-foreground/80 data-active:text-sidebar-accent-foreground data-active:[&>svg]:text-primary [&>svg]:text-muted-foreground hover:[&>svg]:text-sidebar-accent-foreground";

export function AppSidebar({
  sections,
  loading,
  siteTitle,
  siteUrl,
  hidden = false,
}: AppSidebarProps) {
  const { t } = useTranslation("shell");
  const { isMobile, setOpenMobile, state } = useSidebar();
  // Tooltips only name the icons of the collapsed rail. Elsewhere they would be hidden
  // but still open on focus, and the first Escape would close an invisible tooltip
  // instead of the mobile sheet.
  const tip = (label: string) => (state === "collapsed" && !isMobile ? label : undefined);
  const pathname = adminPathname(useLocation({ select: (l) => l.pathname }));
  const close = () => {
    if (isMobile) setOpenMobile(false);
  };

  return (
    <Sidebar collapsible={hidden ? "offcanvas" : "icon"} inert={hidden || undefined}>
      <SidebarHeader className="h-14 justify-center border-b border-sidebar-border px-2 py-0">
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton
              asChild
              size="lg"
              tooltip={tip(siteTitle)}
              className="h-10 gap-2.5 px-1.5 group-data-[collapsible=icon]:p-0! hover:bg-transparent"
            >
              <Link to="/" onClick={close}>
                <span
                  aria-hidden="true"
                  className="flex size-7 shrink-0 items-center justify-center rounded-md bg-primary font-mono text-xs font-bold text-primary-foreground shadow-1"
                >
                  Fe
                </span>
                <span className="flex min-w-0 flex-col leading-tight">
                  <span className="truncate text-sm font-semibold" title={siteTitle}>
                    {siteTitle}
                  </span>
                  <span className="truncate text-xs text-muted-foreground">
                    {t("brand.subtitle")}
                  </span>
                </span>
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>

      <SidebarContent className="py-2">
        <nav aria-label={t("nav.label")}>
          {sections.map((section) => (
            <SidebarGroup key={section.id} className="py-1.5">
              <SidebarGroupLabel className="h-7 text-muted-foreground">
                {section.label}
              </SidebarGroupLabel>
              <SidebarMenu className="gap-0.5">
                {section.items.map((item) => (
                  <SidebarMenuItem key={item.id}>
                    <SidebarMenuButton
                      asChild
                      tooltip={tip(item.label)}
                      isActive={isActive(item, pathname)}
                      className={ROW}
                    >
                      {/* Content routes arrive with Tasks 13–14; until then the
                          not-found screen answers inside the shell. */}
                      <Link to={item.to as "/"} onClick={close}>
                        <item.icon />
                        <span>{item.label}</span>
                        {item.badge ? (
                          <span className="sr-only">{t("nav.badge", { count: item.badge })}</span>
                        ) : null}
                      </Link>
                    </SidebarMenuButton>
                    {item.badge ? (
                      <SidebarMenuBadge
                        aria-hidden="true"
                        className="top-2! tabular-nums pointer-coarse:top-3!"
                      >
                        {item.badge}
                      </SidebarMenuBadge>
                    ) : null}
                  </SidebarMenuItem>
                ))}
              </SidebarMenu>
            </SidebarGroup>
          ))}
          {loading ? (
            <SidebarGroup className="py-1.5" aria-hidden="true">
              <SidebarMenu className="gap-0.5">
                {[0, 1, 2].map((i) => (
                  <SidebarMenuItem key={i}>
                    <SidebarMenuSkeleton showIcon className="h-control" />
                  </SidebarMenuItem>
                ))}
              </SidebarMenu>
            </SidebarGroup>
          ) : null}
        </nav>
      </SidebarContent>

      {siteUrl ? (
        <SidebarFooter className="border-t border-sidebar-border">
          <SidebarMenu>
            <SidebarMenuItem>
              <SidebarMenuButton asChild tooltip={tip(t("nav.viewSite"))} className={ROW}>
                <a href={siteUrl} target="_blank" rel="noopener noreferrer">
                  <ExternalLinkIcon />
                  <span>{t("nav.viewSite")}</span>
                </a>
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
        </SidebarFooter>
      ) : null}
      <SidebarRail />
    </Sidebar>
  );
}
