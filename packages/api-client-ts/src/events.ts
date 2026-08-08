import type { TaskEventEnvelope } from "./client.js";

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const occurredAt = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:Z|[+-](\d{2}):(\d{2}))$/;
const own = (value: object, keys: string[]) => Object.keys(value).length === keys.length && keys.every((key) => Object.hasOwn(value, key));
const nonNegativeInteger = (value: unknown) => Number.isSafeInteger(value) && (value as number) >= 0;
const positiveInteger = (value: unknown) => Number.isSafeInteger(value) && (value as number) >= 1;
const stableReasons = new Set([
  "identification.ambiguous", "identification.confirmed-external-id", "identification.confirmed-title-year",
  "identification.multiple-strong-candidates", "identification.no-candidate", "identification.probable-title",
  "identification.provider-unavailable", "identification.provider-unauthorized", "identification.revision-changed",
  "auxiliary.sample", "auxiliary.trailer", "auxiliary.extra", "watcher.unavailable", "reconcile.failed",
  "integration.unconfigured", "integration.healthy", "integration.unauthorized", "integration.rate-limited", "integration.unavailable",
  "organization.plan-paused", "organization.io-temporary", "organization.manual-review",
  "organization.nfo-failed", "organization.catalog-unavailable",
]);
const stableReason = (value: unknown) => typeof value === "string" && stableReasons.has(value);
const nullableStableReason = (value: unknown) => value === null || stableReason(value);
const integrationFailureCodes = new Set([
  "integration.not-configured", "integration.unauthorized", "integration.rate-limited", "integration.unavailable",
  "provider.timeout", "provider.response-too-large", "provider.invalid-response",
]);
const downloaderFailureCodes = new Set([
  "integration.not-configured", "integration.unauthorized", "integration.rate-limited", "integration.unavailable",
  "integration.unsupported-version", "provider.timeout", "provider.response-too-large", "provider.invalid-response",
  "download.correlation-ambiguous", "download.remote-missing",
]);
const automationFailureCodes = new Set([
  "automation.source-disabled", "automation.signature-invalid", "automation.replay", "automation.action-invalid",
  "automation.payload-invalid", "automation.downstream-conflict", "integration.not-configured",
  "integration.unauthorized", "integration.rate-limited", "integration.unavailable", "provider.timeout",
  "provider.response-too-large", "provider.invalid-response",
]);

function isDateTime(value: string): boolean {
  const match = occurredAt.exec(value);
  if (!match) return false;
  const [year, month, day, hour, minute, second] = match.slice(1, 7).map(Number);
  const offsetHour = Number(match[7] ?? "0");
  const offsetMinute = Number(match[8] ?? "0");
  const calendar = new Date(Date.UTC(year, month - 1, day));
  return calendar.getUTCFullYear() === year && calendar.getUTCMonth() === month - 1 && calendar.getUTCDate() === day && hour <= 23 && minute <= 59 && second <= 59 && offsetHour <= 23 && offsetMinute <= 59;
}

/**
 * 在将第一版任务事件暴露给应用状态前解析并严格验证它。
 *
 * @param value - 来自事件流、不可信的 JSON 兼容值。
 * @returns 字段精确且整数、日期值受限的任一当前受支持 schema-v1 事件信封。
 * @throws {TypeError} 当值不是对象，或不匹配任一受支持的第一版结构时抛出。
 */
export function parseTaskEvent(value: unknown): TaskEventEnvelope {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new TypeError("Task event must be an object");
  const event = value as Record<string, unknown>;
  if (!own(event, ["id", "type", "schema_version", "occurred_at", "task_id", "payload"]) || !Number.isSafeInteger(event.id) || (event.id as number) < 1 || event.schema_version !== "1" || typeof event.occurred_at !== "string" || !isDateTime(event.occurred_at) || !event.payload || typeof event.payload !== "object" || Array.isArray(event.payload)) throw new TypeError("Task event does not match schema version 1");
  const payload = event.payload as Record<string, unknown>;
  if (event.type === "task.progress" && typeof event.task_id === "string" && uuid.test(event.task_id) && own(payload, ["visited_directories", "observed_files", "skipped_entries", "errors"]) && Object.values(payload).every(nonNegativeInteger)) return event as TaskEventEnvelope;
  if (event.type === "task.state-changed" && typeof event.task_id === "string" && uuid.test(event.task_id) && own(payload, ["status", "recovering"]) && ["queued", "running", "partial-success", "completed", "failed", "cancelled"].includes(payload.status as string) && typeof payload.recovering === "boolean") return event as TaskEventEnvelope;
  if (event.type === "processing-task.state-changed" && typeof event.task_id === "string" && uuid.test(event.task_id) && own(payload, ["status", "stage", "recovering", "reason"]) && ["queued", "running", "waiting-confirmation", "paused", "cancelled", "partial-success", "completed", "failed"].includes(payload.status as string) && ["identification", "planning", "file-operation", "nfo", "completion"].includes(payload.stage as string) && typeof payload.recovering === "boolean" && nullableStableReason(payload.reason)) return event as TaskEventEnvelope;
  if (event.type === "processing-task.identification-decided" && typeof event.task_id === "string" && uuid.test(event.task_id) && own(payload, ["decision_id", "level", "reason"]) && typeof payload.decision_id === "string" && uuid.test(payload.decision_id) && ["confirmed", "probable", "ambiguous", "unidentified", "blocked"].includes(payload.level as string) && stableReason(payload.reason)) return event as TaskEventEnvelope;
  if (event.type === "inbox.discovery-health-changed" && event.task_id === null && own(payload, ["inbox_directory_id", "health", "watcher_active", "reason"]) && typeof payload.inbox_directory_id === "string" && uuid.test(payload.inbox_directory_id) && ["healthy", "degraded", "unavailable"].includes(payload.health as string) && typeof payload.watcher_active === "boolean" && nullableStableReason(payload.reason)) return event as TaskEventEnvelope;
  if (event.type === "integration.health-changed" && event.task_id === null && own(payload, ["kind", "health", "failure_code"]) && payload.kind === "tmdb" && ["unconfigured", "healthy", "degraded", "unavailable", "unauthorized", "rate-limited"].includes(payload.health as string) && (payload.failure_code === null || integrationFailureCodes.has(payload.failure_code as string))) return event as TaskEventEnvelope;
  if (event.type === "task-decision.accepted" && typeof event.task_id === "string" && uuid.test(event.task_id) && own(payload, ["case_id", "decision_id", "kind", "case_version"]) && typeof payload.case_id === "string" && uuid.test(payload.case_id) && typeof payload.decision_id === "string" && uuid.test(payload.decision_id) && ["select-provider-candidate", "rematch-with-hints", "select-generic-video"].includes(payload.kind as string) && positiveInteger(payload.case_version)) return event as TaskEventEnvelope;
  if (event.type === "review-case.updated" && typeof event.task_id === "string" && uuid.test(event.task_id) && own(payload, ["case_id", "case_version", "status"]) && typeof payload.case_id === "string" && uuid.test(payload.case_id) && positiveInteger(payload.case_version) && ["active", "closed"].includes(payload.status as string)) return event as TaskEventEnvelope;
  if (event.type === "catalog.media-changed" && event.task_id === null && own(payload, ["media_item_id", "projection_version", "change"]) && typeof payload.media_item_id === "string" && uuid.test(payload.media_item_id) && positiveInteger(payload.projection_version) && ["created", "updated"].includes(payload.change as string)) return event as TaskEventEnvelope;
  if (event.type === "download-task.changed" && typeof event.task_id === "string" && uuid.test(event.task_id) && own(payload, ["projection_version", "status", "remote_status", "progress_basis_points", "failure_code"]) && positiveInteger(payload.projection_version) && ["queued", "submitting", "monitoring", "retry-wait", "completed", "failed"].includes(payload.status as string) && (payload.remote_status === null || ["queued", "downloading", "paused", "completed", "failed", "unknown"].includes(payload.remote_status as string)) && Number.isSafeInteger(payload.progress_basis_points) && (payload.progress_basis_points as number) >= 0 && (payload.progress_basis_points as number) <= 10_000 && (payload.failure_code === null || downloaderFailureCodes.has(payload.failure_code as string))) return event as TaskEventEnvelope;
  if (event.type === "organization-target.changed" && event.task_id === null && own(payload, ["organization_target_id", "projection_version", "change"]) && typeof payload.organization_target_id === "string" && uuid.test(payload.organization_target_id) && positiveInteger(payload.projection_version) && ["created", "updated", "deleted"].includes(payload.change as string)) return event as TaskEventEnvelope;
  if (event.type === "organization-result.changed" && typeof event.task_id === "string" && uuid.test(event.task_id) && own(payload, ["result_id", "projection_version", "status"]) && typeof payload.result_id === "string" && uuid.test(payload.result_id) && positiveInteger(payload.projection_version) && ["partial-success", "completed", "compensated", "manual-review"].includes(payload.status as string)) return event as TaskEventEnvelope;
  if (event.type === "automation-source.changed" && event.task_id === null && own(payload, ["automation_source_id", "projection_version", "change"]) && typeof payload.automation_source_id === "string" && uuid.test(payload.automation_source_id) && positiveInteger(payload.projection_version) && ["created", "updated", "deleted"].includes(payload.change as string)) return event as TaskEventEnvelope;
  if (event.type === "automation-event.changed" && event.task_id === null && own(payload, ["automation_event_id", "projection_version", "status", "action", "failure_code"]) && typeof payload.automation_event_id === "string" && uuid.test(payload.automation_event_id) && positiveInteger(payload.projection_version) && ["pending", "running", "retry-wait", "completed", "failed", "cancelled"].includes(payload.status as string) && ["create-download", "reconcile-inbox"].includes(payload.action as string) && (payload.failure_code === null || automationFailureCodes.has(payload.failure_code as string))) return event as TaskEventEnvelope;
  if (event.type === "identification-enhancer.changed" && event.task_id === null && own(payload, ["projection_version", "enabled", "health", "fallback_code"]) && positiveInteger(payload.projection_version) && typeof payload.enabled === "boolean" && ["unconfigured", "healthy", "degraded", "unavailable", "unauthorized", "rate-limited"].includes(payload.health as string) && (payload.fallback_code === null || automationFailureCodes.has(payload.fallback_code as string))) return event as TaskEventEnvelope;
  if (event.type === "stream.gap" && event.task_id === null && own(payload, ["minimum_available_id"]) && Number.isSafeInteger(payload.minimum_available_id) && (payload.minimum_available_id as number) >= 1) return event as TaskEventEnvelope;
  throw new TypeError("Task event does not match schema version 1");
}

/**
 * 在限制去重内存的同时，仅接受严格递增的正事件 ID。
 *
 * @param seen - 保存最近接受 ID 的可变集合；评估候选 ID 前，格式错误的额外条目会折叠为最大安全整数。
 * @param id - 候选事件 ID。
 * @returns 当候选值为大于先前接受 ID 的安全整数时返回 `true`。
 * @remarks 会修改 `seen`，使其仅包含最新接受的 ID，或者在拒绝时保留先前最大 ID。
 */
export function acceptEventId(seen: Set<number>, id: number): boolean {
  let last = 0;
  for (const candidate of seen) {
    if (Number.isSafeInteger(candidate) && candidate > last) last = candidate;
  }
  seen.clear();
  if (last > 0) seen.add(last);
  if (!Number.isSafeInteger(id) || id < 1 || id <= last) return false;
  seen.clear();
  seen.add(id);
  return true;
}
