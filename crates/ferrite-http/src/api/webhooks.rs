//! `/webhooks` (M6-6, design 18 §8): endpoints, the event catalogue, test deliveries,
//! secret rotation, the delivery log and redelivery.
//!
//! Handlers only translate: `WebhookService` checks `manage_webhooks` with
//! `webhooks:read` (list, get, deliveries, a delivery, the catalogue) or
//! `webhooks:write` (everything else), the URL rules (`422` field `url`), the patterns
//! (`events`), the content types and the secret. The secret is in the create and
//! rotate answers only. Last write wins (no `If-Match`), as for redirects.

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use ferrite_core::webhooks::{self as core, PAYLOAD_VERSION};
use ferrite_domain::{DeliveryId, DeliveryQuery, MAX_PAGE, Secret, Timestamp, WebhookId};
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use super::{
    ApiState, Authed, BASE,
    dto::webhooks::{
        CreatedWebhookDto, DeliveryDetailDto, DeliveryDto, DeliveryList, DeliveryStatusDto,
        EventCatalogueDto, QueuedDeliveryDto, RedeliverInput, RedeliveredDto, RotatedSecretDto,
        WebhookDto, WebhookInput, WebhookList, WebhookPatch,
    },
    extract::{ApiJson, ApiPath, ApiQuery, DEFAULT_LIMIT, check_limit, created_at, invalid_cursor},
    list,
    problem::{Problem, ProblemDocument},
};

#[utoipa::path(
    get,
    path = "/webhooks",
    tag = "webhooks",
    operation_id = "list_webhooks",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "Every webhook, by name", body = WebhookList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_webhooks`, `webhooks:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_webhooks(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<WebhookList>, Problem> {
    let items = api.webhooks.list(&who.actor).await?;
    Ok(Json(WebhookList {
        items: items.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/webhooks",
    tag = "webhooks",
    operation_id = "create_webhook",
    security(("session" = []), ("bearer" = [])),
    request_body = WebhookInput,
    responses(
        (status = 201, description = "Created (`Location`); the secret is shown this once", body = CreatedWebhookDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_webhooks`, `webhooks:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field`: `name`, `url`, `events`, `content_types` or `secret`), or at most 20 webhooks", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn create_webhook(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiJson(input): ApiJson<WebhookInput>,
) -> Result<Response, Problem> {
    let (view, secret) = api
        .webhooks
        .create(
            &who.actor,
            core::WebhookInput {
                name: input.name,
                url: input.url,
                events: input.events,
                content_types: input.content_types,
                enabled: input.enabled,
                secret: input.secret.map(Secret::new),
            },
        )
        .await?;
    let (status, headers) = created_at(&format!("{BASE}/webhooks/{}", view.endpoint.id));
    let body = CreatedWebhookDto {
        webhook: view.into(),
        secret: secret.expose().to_owned(),
    };
    Ok((status, headers, Json(body)).into_response())
}

#[utoipa::path(
    get,
    path = "/webhooks/events",
    tag = "webhooks",
    operation_id = "list_webhook_events",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "The event types endpoints can subscribe to", body = EventCatalogueDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_webhooks`, `webhooks:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_webhook_events(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<EventCatalogueDto>, Problem> {
    let types = api.webhooks.event_types(&who.actor)?;
    Ok(Json(EventCatalogueDto {
        version: PAYLOAD_VERSION,
        types: types.iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/webhooks/{id}",
    tag = "webhooks",
    operation_id = "get_webhook",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The webhook", body = WebhookDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_webhooks`, `webhooks:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_webhook(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<WebhookDto>, Problem> {
    let view = api
        .webhooks
        .get(&who.actor, WebhookId::from_uuid(id))
        .await?;
    Ok(Json(view.into()))
}

#[utoipa::path(
    patch,
    path = "/webhooks/{id}",
    tag = "webhooks",
    operation_id = "update_webhook",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    request_body = WebhookPatch,
    responses(
        (status = 200, description = "Saved", body = WebhookDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_webhooks`, `webhooks:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field`: `name`, `url`, `events` or `content_types`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn update_webhook(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(patch): ApiJson<WebhookPatch>,
) -> Result<Json<WebhookDto>, Problem> {
    let view = api
        .webhooks
        .update(
            &who.actor,
            WebhookId::from_uuid(id),
            core::WebhookPatch {
                name: patch.name,
                url: patch.url,
                events: patch.events,
                content_types: patch.content_types,
                enabled: patch.enabled,
            },
        )
        .await?;
    Ok(Json(view.into()))
}

#[utoipa::path(
    delete,
    path = "/webhooks/{id}",
    tag = "webhooks",
    operation_id = "delete_webhook",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 204, description = "Deleted, with its deliveries"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_webhooks`, `webhooks:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_webhook(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    api.webhooks
        .delete(&who.actor, WebhookId::from_uuid(id))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/webhooks/{id}/secret",
    tag = "webhooks",
    operation_id = "rotate_webhook_secret",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "A new secret, shown this once; the old one signs too for 24 hours", body = RotatedSecretDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_webhooks`, `webhooks:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn rotate_webhook_secret(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<RotatedSecretDto>, Problem> {
    let (secret, until) = api
        .webhooks
        .rotate_secret(&who.actor, WebhookId::from_uuid(id))
        .await?;
    Ok(Json(RotatedSecretDto {
        secret: secret.expose().to_owned(),
        previous_valid_until: until.as_datetime(),
    }))
}

#[utoipa::path(
    post,
    path = "/webhooks/{id}/test",
    tag = "webhooks",
    operation_id = "test_webhook",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 202, description = "A `webhook.ping` delivery is queued (poll it)", body = QueuedDeliveryDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_webhooks`, `webhooks:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: paused (a copied or restored database)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 429, description = "`rate-limited`: 10 test deliveries per user and minute (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn test_webhook(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<(StatusCode, Json<QueuedDeliveryDto>), Problem> {
    let delivery = api
        .webhooks
        .test(&who.actor, WebhookId::from_uuid(id))
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(QueuedDeliveryDto {
            delivery: delivery.as_uuid(),
        }),
    ))
}

/// The query of `GET /webhooks/{id}/deliveries`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DeliveryListParams {
    /// Only deliveries in this status.
    pub status: Option<DeliveryStatusDto>,
    /// 1..=200; default 50.
    pub limit: Option<u32>,
    /// `next_cursor` of the previous page of the same query.
    pub cursor: Option<String>,
}

#[utoipa::path(
    get,
    path = "/webhooks/{id}/deliveries",
    tag = "webhooks",
    operation_id = "list_webhook_deliveries",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), DeliveryListParams),
    responses(
        (status = 200, description = "One page of deliveries, newest first", body = DeliveryList),
        (status = 400, description = "`validation`: `limit`, `cursor` or `status`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_webhooks`, `webhooks:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_webhook_deliveries(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<DeliveryListParams>,
) -> Result<Json<DeliveryList>, Problem> {
    let limit = check_limit(params.limit, DEFAULT_LIMIT, MAX_PAGE)?;
    if let Some(c) = &params.cursor
        && ferrite_store::webhook_delivery_cursor(c).is_err()
    {
        return Err(invalid_cursor());
    }
    let page = api
        .webhooks
        .deliveries(
            &who.actor,
            WebhookId::from_uuid(id),
            &DeliveryQuery {
                status: params.status.map(Into::into),
                limit,
                cursor: params.cursor,
            },
        )
        .await?;
    let (items, next_cursor) = list::items(page, DeliveryDto::from);
    Ok(Json(DeliveryList { items, next_cursor }))
}

#[utoipa::path(
    get,
    path = "/webhooks/{id}/deliveries/{delivery}",
    tag = "webhooks",
    operation_id = "get_webhook_delivery",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("delivery" = Uuid, Path)),
    responses(
        (status = 200, description = "The delivery with its event and attempts", body = DeliveryDetailDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_webhooks`, `webhooks:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found` (also a delivery of another webhook)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_webhook_delivery(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((id, delivery)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<DeliveryDetailDto>, Problem> {
    let detail = api
        .webhooks
        .delivery(
            &who.actor,
            WebhookId::from_uuid(id),
            DeliveryId::from_uuid(delivery),
        )
        .await?;
    Ok(Json(detail.into()))
}

#[utoipa::path(
    post,
    path = "/webhooks/{id}/deliveries/{delivery}/redeliver",
    tag = "webhooks",
    operation_id = "redeliver_webhook_delivery",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("delivery" = Uuid, Path)),
    responses(
        (status = 202, description = "A new delivery of the same event is queued", body = QueuedDeliveryDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_webhooks`, `webhooks:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: the webhook is disabled or paused", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn redeliver_webhook_delivery(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((id, delivery)): ApiPath<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<QueuedDeliveryDto>), Problem> {
    let queued = api
        .webhooks
        .redeliver(
            &who.actor,
            WebhookId::from_uuid(id),
            DeliveryId::from_uuid(delivery),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(QueuedDeliveryDto {
            delivery: queued.as_uuid(),
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/webhooks/{id}/redeliver",
    tag = "webhooks",
    operation_id = "redeliver_webhook",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    request_body = RedeliverInput,
    responses(
        (status = 202, description = "Deliveries since the time queued again (at most 1 000, oldest first)", body = RedeliveredDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_webhooks`, `webhooks:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: `statuses` (`pending`), or the webhook is disabled or paused", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn redeliver_webhook(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(input): ApiJson<RedeliverInput>,
) -> Result<(StatusCode, Json<RedeliveredDto>), Problem> {
    let count = api
        .webhooks
        .redeliver_since(
            &who.actor,
            WebhookId::from_uuid(id),
            Timestamp::from(input.since),
            input
                .statuses
                .map(|s| s.into_iter().map(Into::into).collect()),
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(RedeliveredDto { count })))
}
