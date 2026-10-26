// One comment of the queue (07 §3 Comments; M2b-2 Task 12): who wrote it (name, e-mail,
// website, a short form of the keyed address hash), when, what it replies to, the body
// and the item it is on. Everything the commenter typed is untrusted and rendered as
// text nodes only (Review Focus 3): the body keeps its line breaks with
// `white-space: pre-wrap`, never through HTML, and the website is a link only for an
// `http:`/`https:` address (`rel="noopener noreferrer nofollow ugc"`, new tab); anything
// else shows as plain text. Long bodies are clamped to 6 lines with Show more.

import { Link } from "@tanstack/react-router";
import { ExternalLinkIcon, CornerDownRightIcon, ListFilterIcon } from "lucide-react";
import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { CommentDto } from "@/api/comments";
import { RelativeTime } from "@/components/kit/relative-time";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { editPath } from "@/content/list-columns";
import { authorLink } from "./author-link";

/** How many characters of `ip_hash` the row shows (the full hash is in `title`). */
const IP_HASH_SHOWN = 8;

/** An address without its scheme and trailing slash (`ada.example/blog`). */
function shortUrl(url: string): string {
  return url.replace(/^https?:\/\//i, "").replace(/\/$/, "");
}

export interface CommentRowProps {
  comment: CommentDto;
  /**
   * The replied-to comment's author when it is loaded, `null` when it is not (the row
   * still says it is a reply). Ignored for a comment without `parent_id`.
   */
  parentName?: string | null;
  /** Narrows the queue to this comment's item (the item filter). */
  onFilterItem?: () => void;
  /** The row's action buttons. */
  actions?: ReactNode;
  /** Below the actions (the reply box). */
  footer?: ReactNode;
}

export function CommentRow({
  comment,
  parentName = null,
  onFilterItem,
  actions,
  footer,
}: CommentRowProps) {
  const { t } = useTranslation("comments");
  const href = comment.author_url ? authorLink(comment.author_url) : null;
  const title = comment.item.title || t("row.untitled");
  const hash = comment.ip_hash ?? "";
  return (
    <article
      data-slot="comment"
      aria-label={comment.author_name}
      className="flex min-w-0 flex-col gap-2 py-3 whitespace-normal"
    >
      <header className="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-1">
        <span className="max-w-full font-medium break-words text-foreground">
          {comment.author_name}
        </span>
        {comment.user_id ? (
          <Badge variant="secondary" className="self-center">
            {t("row.account")}
          </Badge>
        ) : null}
        <a
          href={`mailto:${comment.author_email}`}
          className="max-w-full truncate font-mono text-xs text-muted-foreground hover:text-foreground hover:underline"
          title={comment.author_email}
        >
          {comment.author_email}
        </a>
        {comment.author_url ? (
          href ? (
            <a
              href={href}
              target="_blank"
              rel="noopener noreferrer nofollow ugc"
              title={t("row.website", { url: comment.author_url })}
              className="inline-flex max-w-full min-w-0 items-center gap-1 text-xs text-muted-foreground hover:text-foreground hover:underline"
            >
              <span className="truncate">{shortUrl(comment.author_url)}</span>
              <ExternalLinkIcon aria-hidden="true" className="size-3 shrink-0" />
            </a>
          ) : (
            <span
              data-slot="comment-url-text"
              className="max-w-full truncate text-xs text-muted-foreground"
              title={t("row.website", { url: comment.author_url })}
            >
              {comment.author_url}
            </span>
          )
        ) : null}
        {hash ? (
          <span
            className="font-mono text-xs text-muted-foreground"
            title={t("row.address", { hash })}
          >
            {hash.slice(0, IP_HASH_SHOWN)}
          </span>
        ) : null}
        <RelativeTime
          at={comment.created_at}
          className="ml-auto text-xs whitespace-nowrap text-muted-foreground"
        />
      </header>

      {comment.parent_id ? (
        <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <CornerDownRightIcon aria-hidden="true" className="size-3.5 shrink-0" />
          <span className="min-w-0 break-words">
            {parentName !== null ? t("row.replyTo", { name: parentName }) : t("row.replyToUnknown")}
          </span>
        </p>
      ) : null}

      <CommentBody body={comment.body} />

      <p className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-sm text-muted-foreground">
        <span>{t("row.on")}</span>
        <Link
          to={editPath(comment.item.type, comment.item.id) as "/"}
          className="max-w-full truncate font-medium text-foreground hover:underline"
          title={title}
        >
          {title}
        </Link>
        {comment.item.permalink ? (
          <a
            href={comment.item.permalink}
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex items-center gap-1 hover:text-foreground hover:underline"
          >
            {t("row.view")}
            <ExternalLinkIcon aria-hidden="true" className="size-3" />
            <span className="sr-only">{t("row.newTab")}</span>
          </a>
        ) : null}
        {onFilterItem ? (
          <Button
            variant="ghost"
            size="icon-xs"
            className="text-muted-foreground pointer-coarse:size-control"
            aria-label={t("filter.only", { title })}
            title={t("filter.only", { title })}
            onClick={onFilterItem}
          >
            <ListFilterIcon />
          </Button>
        ) : null}
      </p>

      {actions}
      {footer}
    </article>
  );
}

/** The body as text, clamped to 6 lines; Show more when it is longer. */
function CommentBody({ body }: { body: string }) {
  const { t } = useTranslation("comments");
  const ref = useRef<HTMLParagraphElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [overflows, setOverflows] = useState(false);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || expanded) return;
    const check = () => setOverflows(el.scrollHeight > el.clientHeight + 1);
    check();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(check);
    ro.observe(el);
    return () => ro.disconnect();
  }, [body, expanded]);

  return (
    <div className="flex flex-col items-start gap-1">
      <p
        ref={ref}
        data-slot="comment-body"
        data-expanded={expanded || undefined}
        className={
          expanded
            ? "max-w-prose text-base break-words whitespace-pre-wrap text-foreground"
            : "line-clamp-6 max-w-prose text-base break-words whitespace-pre-wrap text-foreground"
        }
      >
        {body}
      </p>
      {overflows || expanded ? (
        <Button
          variant="link"
          size="sm"
          className="h-auto px-0 pointer-coarse:h-control"
          aria-expanded={expanded}
          onClick={() => setExpanded((v) => !v)}
        >
          {expanded ? t("row.showLess") : t("row.showMore")}
        </Button>
      ) : null}
    </div>
  );
}
