// Test rows for the webhook screens, shaped as the API answers.

import type { DeliveryDetailDto, DeliveryDto, EventCatalogueDto, WebhookDto } from "@/api/webhooks";

export const HOOK_ID = "0196f6b0-0000-7000-8000-0000000000a1";
export const DELIVERY_ID = "0196f6b0-0000-7000-8000-0000000000d1";
export const EVENT_ID = "0196f6b0-0000-7000-8000-0000000000e1";

export function webhook(over: Partial<WebhookDto> = {}): WebhookDto {
  return {
    id: HOOK_ID,
    name: "Deploy",
    url: "https://hooks.example.com/build?token=s3cr3t",
    events: ["content.*"],
    content_types: null,
    enabled: true,
    state: "active",
    disabled_reason: null,
    disabled_at: null,
    failing_since: null,
    consecutive_failures: 0,
    last_success_at: "2026-11-02T10:00:00Z",
    last_failure_at: null,
    payload_version: 1,
    secret_rotating_until: null,
    created_at: "2026-11-01T10:00:00Z",
    updated_at: "2026-11-01T10:00:00Z",
    counts_24h: { delivered: 3, failed: 1, pending: 0, skipped: 0 },
    ...over,
  };
}

export const CATALOGUE: EventCatalogueDto = {
  version: 1,
  types: [
    { type: "content.created", group: "content", description: "A content item was created." },
    { type: "content.published", group: "content", description: "A content item was published." },
    { type: "media.uploaded", group: "media", description: "A file was uploaded." },
    { type: "user.created", group: "user", description: "A user account was created." },
  ],
};

export const TYPES = {
  items: [
    { name: "post", label: "Posts", built_in: true, hierarchical: false, has_archive: true },
    { name: "page", label: "Pages", built_in: true, hierarchical: true, has_archive: false },
    { name: "media", label: "Media", built_in: true, hierarchical: false, has_archive: false },
  ],
};

export function delivery(over: Partial<DeliveryDto> = {}): DeliveryDto {
  return {
    id: DELIVERY_ID,
    event_id: EVENT_ID,
    event_type: "content.published",
    status: "failed",
    attempts: 1,
    last_status_code: 500,
    last_error: "status 500",
    next_attempt_at: null,
    created_at: "2026-11-02T09:00:00Z",
    finished_at: "2026-11-02T09:00:01Z",
    redelivery_of: null,
    ...over,
  };
}

export function detail(over: Partial<DeliveryDetailDto> = {}): DeliveryDetailDto {
  return {
    delivery: delivery(),
    event: {
      id: EVENT_ID,
      type: "content.published",
      timestamp: "2026-11-02T09:00:00Z",
      data: { id: "x", title: "Hello" },
    },
    attempts: [
      {
        attempt: 1,
        started_at: "2026-11-02T09:00:00Z",
        duration_ms: 42,
        status_code: 500,
        error: "status 500",
        response_excerpt: "<b>broken</b>",
      },
    ],
    ...over,
  };
}
