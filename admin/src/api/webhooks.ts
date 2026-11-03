// `/webhooks` (M6-6, design 18 §8; `manage_webhooks` with `webhooks:read`/`webhooks:write`).
// The create and rotate answers carry a secret shown once: they are plain calls whose
// result the caller keeps in component state, never in the query cache.

import { infiniteQueryOptions, queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type {
  SchemaCreatedWebhookDto,
  SchemaDeliveryDetailDto,
  SchemaDeliveryDto,
  SchemaDeliveryList,
  SchemaDeliveryStatusDto,
  SchemaEventCatalogueDto,
  SchemaRotatedSecretDto,
  SchemaWebhookDto,
  SchemaWebhookInput,
  SchemaWebhookPatch,
} from "./schema";

export type WebhookDto = SchemaWebhookDto;
export type DeliveryDto = SchemaDeliveryDto;
export type DeliveryDetailDto = SchemaDeliveryDetailDto;
export type DeliveryStatus = SchemaDeliveryStatusDto;
export type EventCatalogueDto = SchemaEventCatalogueDto;
/** Carries the secret: component state only. */
export type CreatedWebhookDto = SchemaCreatedWebhookDto;
/** Carries the secret: component state only. */
export type RotatedSecretDto = SchemaRotatedSecretDto;
export type WebhookInput = SchemaWebhookInput;
export type WebhookPatch = SchemaWebhookPatch;

/** Endpoints a site may have (the server's `422` past it). */
export const MAX_WEBHOOKS = 20;
/** One page of the delivery log. */
export const DELIVERY_PAGE = 50;

export async function listWebhooks(): Promise<WebhookDto[]> {
  const list = await unwrap(api.GET("/api/v1/webhooks"));
  return list.items;
}

export const webhooksQuery = queryOptions({
  queryKey: qk.webhooks.list,
  queryFn: listWebhooks,
  staleTime: 15_000,
});

export function getWebhook(id: string): Promise<WebhookDto> {
  return unwrap(api.GET("/api/v1/webhooks/{id}", { params: { path: { id } } }));
}

export function webhookQuery(id: string) {
  return queryOptions({
    queryKey: qk.webhooks.item(id),
    queryFn: () => getWebhook(id),
    staleTime: 15_000,
  });
}

export const eventCatalogueQuery = queryOptions({
  queryKey: qk.webhooks.events,
  queryFn: () => unwrap(api.GET("/api/v1/webhooks/events")),
  staleTime: 5 * 60_000,
});

/** `POST /webhooks`: the answer's `secret` is shown once. */
export function createWebhook(body: WebhookInput): Promise<CreatedWebhookDto> {
  return unwrap(api.POST("/api/v1/webhooks", { body }));
}

export function patchWebhook(id: string, body: WebhookPatch): Promise<WebhookDto> {
  return unwrap(api.PATCH("/api/v1/webhooks/{id}", { params: { path: { id } }, body }));
}

export async function deleteWebhook(id: string): Promise<void> {
  await unwrap(api.DELETE("/api/v1/webhooks/{id}", { params: { path: { id } } }));
}

/** `POST /webhooks/{id}/secret`: the new secret, shown once. */
export function rotateSecret(id: string): Promise<RotatedSecretDto> {
  return unwrap(api.POST("/api/v1/webhooks/{id}/secret", { params: { path: { id } } }));
}

/** `POST /webhooks/{id}/test`: the queued delivery's id. */
export async function testWebhook(id: string): Promise<string> {
  const queued = await unwrap(api.POST("/api/v1/webhooks/{id}/test", { params: { path: { id } } }));
  return queued.delivery;
}

export function listDeliveries(
  id: string,
  query: { status?: DeliveryStatus; cursor?: string; limit?: number },
): Promise<SchemaDeliveryList> {
  return unwrap(api.GET("/api/v1/webhooks/{id}/deliveries", { params: { path: { id }, query } }));
}

/** The delivery log's pages, newest first (`useInfiniteQuery` over `next_cursor`). */
export function deliveriesQuery(id: string, status: DeliveryStatus | undefined) {
  return infiniteQueryOptions({
    queryKey: qk.webhooks.deliveries(id, status ?? "all"),
    queryFn: ({ pageParam }) =>
      listDeliveries(id, {
        limit: DELIVERY_PAGE,
        ...(status ? { status } : {}),
        ...(pageParam ? { cursor: pageParam } : {}),
      }),
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    staleTime: 5_000,
  });
}

export function getDelivery(id: string, delivery: string): Promise<DeliveryDetailDto> {
  return unwrap(
    api.GET("/api/v1/webhooks/{id}/deliveries/{delivery}", {
      params: { path: { id, delivery } },
    }),
  );
}

export function deliveryQuery(id: string, delivery: string) {
  return queryOptions({
    queryKey: qk.webhooks.delivery(id, delivery),
    queryFn: () => getDelivery(id, delivery),
    staleTime: 2_000,
  });
}

/** A new delivery of the same event; its id. */
export async function redeliver(id: string, delivery: string): Promise<string> {
  const queued = await unwrap(
    api.POST("/api/v1/webhooks/{id}/deliveries/{delivery}/redeliver", {
      params: { path: { id, delivery } },
    }),
  );
  return queued.delivery;
}

/** Redelivers the failed and skipped deliveries since `since` (RFC 3339); how many. */
export async function redeliverSince(id: string, since: string): Promise<number> {
  const done = await unwrap(
    api.POST("/api/v1/webhooks/{id}/redeliver", {
      params: { path: { id } },
      body: { since },
    }),
  );
  return done.count;
}
