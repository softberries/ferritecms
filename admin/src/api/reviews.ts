// `/reviews` and `/workflows` (M6-3, design 15 §6, §10): a content type's approval steps
// (admins), and the reviews of items and release entries: the queue, a review's steps,
// decisions and assignees, deciding, assigning and withdrawing. Who may decide is the
// server's call; the admin offers the actions to editors and admins and shows refusals.

import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { ifMatch } from "./content";
import type {
  SchemaApproverDto,
  SchemaAssigneeDto,
  SchemaDecisionDto,
  SchemaReviewDto,
  SchemaReviewList,
  SchemaReviewSummaryDto,
  SchemaWorkflowDto,
  SchemaWorkflowStepDto,
} from "./schema";

export type ReviewDto = SchemaReviewDto;
export type ReviewSummary = SchemaReviewSummaryDto;
export type DecisionDto = SchemaDecisionDto;
export type AssigneeDto = SchemaAssigneeDto;
export type WorkflowDto = SchemaWorkflowDto;
export type WorkflowStep = SchemaWorkflowStepDto;
export type ApproverDto = SchemaApproverDto;

export const MAX_STEPS = 5;
export const MAX_REQUIRED = 5;
export const MAX_STEP_USERS = 20;
export const MAX_ASSIGNEES = 10;
export const MAX_LABEL = 80;
export const MAX_COMMENT = 2000;

export type QueueTab = "assigned" | "open" | "mine";

export const reviewKeys = {
  all: ["reviews"] as const,
  lists: ["reviews", "list"] as const,
  list: (tab: string) => ["reviews", "list", tab] as const,
  item: (id: string) => ["reviews", "item", id] as const,
};

export const workflowKeys = {
  all: ["workflows"] as const,
  item: (type: string) => ["workflows", type] as const,
  approvers: (type: string, step: number) => ["workflows", type, "approvers", step] as const,
};

export function listReviews(tab: QueueTab, limit = 100): Promise<SchemaReviewList> {
  const query = tab === "assigned" ? { assigned: "me" } : tab === "mine" ? { submitted: "me" } : {};
  return unwrap(api.GET("/api/v1/reviews", { params: { query: { ...query, limit } } }));
}

export function reviewsQuery(tab: QueueTab, limit = 100) {
  return queryOptions({
    queryKey: [...reviewKeys.list(tab), limit] as const,
    queryFn: () => listReviews(tab, limit),
    staleTime: 10_000,
  });
}

export function getReview(id: string): Promise<ReviewDto> {
  return unwrap(api.GET("/api/v1/reviews/{id}", { params: { path: { id } } }));
}

export function reviewQuery(id: string) {
  return queryOptions({
    queryKey: reviewKeys.item(id),
    queryFn: () => getReview(id),
    staleTime: 2_000,
  });
}

export function decide(
  id: string,
  version: number,
  decision: "approve" | "request_changes",
  comment?: string,
): Promise<ReviewDto> {
  return unwrap(
    api.POST("/api/v1/reviews/{id}/decisions", {
      params: { path: { id }, header: ifMatch(version) },
      body: { decision, ...(comment ? { comment } : {}) },
    }),
  );
}

export async function assign(id: string, step: number, users: string[]): Promise<AssigneeDto[]> {
  const list = await unwrap(
    api.PUT("/api/v1/reviews/{id}/assignees", { params: { path: { id } }, body: { step, users } }),
  );
  return list.items;
}

export function withdraw(id: string, version: number): Promise<ReviewDto> {
  return unwrap(
    api.POST("/api/v1/reviews/{id}/withdraw", {
      params: { path: { id }, header: ifMatch(version) },
    }),
  );
}

/** Who may approve `step` of the type's workflow: anyone who may submit may ask. */
export async function listApprovers(type: string, step: number): Promise<ApproverDto[]> {
  const list = await unwrap(
    api.GET("/api/v1/workflows/{type}/approvers", {
      params: { path: { type }, query: { step } },
    }),
  );
  return list.items;
}

export function approversQuery(type: string, step: number) {
  return queryOptions({
    queryKey: workflowKeys.approvers(type, step),
    queryFn: () => listApprovers(type, step),
    staleTime: 60_000,
  });
}

/**
 * The type's workflow, or `null` when it has none. Read from the list: a type without one
 * is a `404` on `GET /workflows/{type}`, which the browser would log as an error.
 */
export async function getWorkflow(type: string): Promise<WorkflowDto | null> {
  const list = await unwrap(api.GET("/api/v1/workflows"));
  return list.items.find((w) => w.type === type) ?? null;
}

export function workflowQuery(type: string) {
  return queryOptions({
    queryKey: workflowKeys.item(type),
    queryFn: () => getWorkflow(type),
    staleTime: 10_000,
  });
}

/** Creates or replaces the workflow (`version`: the current one's, when there is one). */
export function putWorkflow(
  type: string,
  steps: WorkflowStep[],
  version: number | null,
): Promise<WorkflowDto> {
  return unwrap(
    api.PUT("/api/v1/workflows/{type}", {
      params: { path: { type }, ...(version === null ? {} : { header: ifMatch(version) }) },
      body: { steps },
    }),
  );
}

export async function deleteWorkflow(type: string, version: number): Promise<void> {
  await unwrap(
    api.DELETE("/api/v1/workflows/{type}", {
      params: { path: { type }, header: ifMatch(version) },
    }),
  );
}

/** The preset (design 15 §6.1): one step, editors and admins, one approval. */
export function oneEditorApproves(label: string): WorkflowStep[] {
  return [
    { key: "review", label, approvers: { roles: ["editor", "admin"], users: [] }, required: 1 },
  ];
}

/** Why a set of steps would be refused (by index and member), else an empty object. */
export function stepIssues(steps: readonly WorkflowStep[]): Record<string, string> {
  const out: Record<string, string> = {};
  if (steps.length === 0) out.steps = "none";
  if (steps.length > MAX_STEPS) out.steps = "tooMany";
  const keys = new Set<string>();
  steps.forEach((s, i) => {
    const label = s.label.trim();
    if (label === "" || [...label].length > MAX_LABEL) out[`${i}.label`] = "label";
    if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(s.key) || keys.has(s.key)) out[`${i}.key`] = "key";
    keys.add(s.key);
    if (!Number.isInteger(s.required) || s.required < 1 || s.required > MAX_REQUIRED) {
      out[`${i}.required`] = "required";
    }
    const roles = s.approvers.roles ?? [];
    const users = s.approvers.users ?? [];
    if (roles.length === 0 && users.length === 0) out[`${i}.approvers`] = "approvers";
    if (users.length > MAX_STEP_USERS) out[`${i}.approvers`] = "tooManyUsers";
  });
  return out;
}

/** A step key from its label (a slug), unique among `taken`. */
export function keyFor(label: string, taken: ReadonlySet<string>): string {
  const base =
    label
      .toLowerCase()
      .normalize("NFKD")
      .replace(/[̀-ͯ]/g, "")
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 40) || "step";
  let key = base;
  for (let n = 2; taken.has(key); n++) key = `${base}-${n}`;
  return key;
}
