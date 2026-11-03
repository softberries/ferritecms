// What the editor does with the server's findings (design 16 §11.1): where each one is
// in the ProseMirror document, its words, and the panel's grouping.

import type { Node as PMNode } from "@tiptap/pm/model";
import type { TFunction } from "i18next";
import type { FindingDto, RuleId } from "@/api/quality";

export type Severity = FindingDto["severity"];

/** The severities, most serious first. */
export const SEVERITIES: readonly Severity[] = ["error", "warning", "notice"];

/**
 * The position just before the node at `path` (child indices from the document root,
 * as the server sends them), or `null` when the document no longer has it.
 */
export function findingPos(doc: PMNode, path: readonly number[]): number | null {
  let node: PMNode = doc;
  // Inside the document, content starts at 0; inside any other node, after its opening.
  let pos = 0;
  let start = 0;
  for (const index of path) {
    if (!Number.isInteger(index) || index < 0 || index >= node.childCount) return null;
    let offset = start;
    for (let i = 0; i < index; i += 1) offset += node.child(i).nodeSize;
    pos = offset;
    node = node.child(index);
    start = pos + 1;
  }
  return path.length === 0 ? null : pos;
}

/** A stable id of a finding within one report (rule, key and location). */
export function findingId(f: FindingDto): string {
  return `${f.rule}|${f.key}|${f.location.kind}|${f.location.field ?? ""}|${(f.location.path ?? []).join(".")}`;
}

/** The sentence the admin shows for a finding (one i18n key per rule). */
export function messageFor(t: TFunction<"quality">, f: { rule: RuleId; context?: string | null }) {
  return t(`rules.${f.rule}.message`, { found: f.context ?? "" });
}

/** The short label of a rule. */
export function titleFor(t: TFunction<"quality">, rule: RuleId) {
  return t(`rules.${rule}.title`);
}

/** Unsuppressed findings by severity (most serious first) and the suppressed ones. */
export function groupFindings(findings: readonly FindingDto[]): {
  bySeverity: { severity: Severity; findings: FindingDto[] }[];
  suppressed: FindingDto[];
} {
  const suppressed = findings.filter((f) => f.suppressed);
  const open = findings.filter((f) => !f.suppressed);
  return {
    bySeverity: SEVERITIES.map((severity) => ({
      severity,
      findings: open.filter((f) => f.severity === severity),
    })).filter((g) => g.findings.length > 0),
    suppressed,
  };
}

/** The accessibility errors a gated publish refuses (unsuppressed). */
export function blockingFindings(findings: readonly FindingDto[]): FindingDto[] {
  return findings.filter(
    (f) => !f.suppressed && f.severity === "error" && f.category === "accessibility",
  );
}
