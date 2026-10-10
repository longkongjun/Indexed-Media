import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import Ajv2020, { type AnySchema } from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

const root = new URL("../../../", import.meta.url);
const loadJson = (path: string) => JSON.parse(readFileSync(new URL(path, root), "utf8")) as unknown;
const schemaPath = (name: string) => `contracts/capability/v1/${name}.schema.json`;
const examplePath = (name: string) => `contracts/examples/v1/capability/${name}.json`;

function compile(schemaName: string) {
  const ajv = new Ajv2020({ allErrors: true, strict: false });
  addFormats(ajv);
  // 信封间存在 $ref 交叉引用（result/event 引用 error schema），一起注册；
  // 目标 schema 自身交给 compile 注册，避免重复 $id 报错。
  for (const dependency of ["capability-error", "capability-event"]) {
    if (dependency === schemaName) continue;
    ajv.addSchema(loadJson(schemaPath(dependency)) as AnySchema);
  }
  return ajv.compile(loadJson(schemaPath(schemaName)) as AnySchema);
}

describe("media-capability.v1 contract fixtures", () => {
  it("validates capability call envelopes for both read-only and plan-only modes", () => {
    const validate = compile("capability-call");
    for (const name of ["capability-call.read-only", "capability-call.plan-only"]) {
      const example = loadJson(examplePath(name));
      expect(validate(example), `${name}: ${JSON.stringify(validate.errors)}`).toBe(true);
    }
  });

  it("validates result envelopes including needs_approval with next actions", () => {
    const validate = compile("capability-result");
    for (const name of ["capability-result.succeeded", "capability-result.needs-approval", "capability-result.rejected.stale-revision"]) {
      const example = loadJson(examplePath(name));
      expect(validate(example), `${name}: ${JSON.stringify(validate.errors)}`).toBe(true);
    }
  });

  it("validates structured error envelopes with machine-judgable fields", () => {
    const validate = compile("capability-error");
    const result = loadJson(examplePath("capability-result.rejected.stale-revision")) as { error: unknown };
    expect(validate(result.error), JSON.stringify(validate.errors)).toBe(true);
  });

  it("validates capability events and cursor pages with monotonic sequences", () => {
    const eventValidate = compile("capability-event");
    for (const name of ["capability-event.waiting-approval", "capability-event.external-sync-retryable"]) {
      const example = loadJson(examplePath(name));
      expect(eventValidate(example), `${name}: ${JSON.stringify(eventValidate.errors)}`).toBe(true);
    }
    const pageValidate = compile("capability-event-page");
    const page = loadJson(examplePath("capability-event-page")) as { items: Array<{ sequence: number }>; next_cursor: string | null };
    expect(pageValidate(page), JSON.stringify(pageValidate.errors)).toBe(true);
    const sequences = page.items.map((item) => item.sequence);
    expect([...sequences].sort((a, b) => a - b)).toEqual(sequences);
    expect(page.next_cursor).toBe("capability-events:43");
  });

  it("rejects unknown capability names, execution modes, and event types", () => {
    const callValidate = compile("capability-call");
    const call = loadJson(examplePath("capability-call.read-only")) as Record<string, unknown>;
    for (const mutation of [
      (value: Record<string, unknown>) => { value.capability = "media.explode"; },
      (value: Record<string, unknown>) => { (value.execution as Record<string, unknown>).mode = "irreversible-write"; },
      (value: Record<string, unknown>) => { value.protocol_version = "media-capability.v2"; },
    ]) {
      const candidate = structuredClone(call);
      mutation(candidate);
      expect(callValidate(candidate), JSON.stringify(callValidate.errors)).toBe(false);
    }

    const eventValidate = compile("capability-event");
    const event = loadJson(examplePath("capability-event.waiting-approval")) as Record<string, unknown>;
    event.event_type = "task.exploded";
    expect(eventValidate(event)).toBe(false);
  });

  it("rejects unsafe idempotency keys and oversized free-form inputs", () => {
    const validate = compile("capability-call");
    const call = loadJson(examplePath("capability-call.read-only")) as { execution: Record<string, unknown>; input: Record<string, unknown> };
    const badKey = structuredClone(call);
    badKey.execution.idempotency_key = "../escape";
    expect(validate(badKey)).toBe(false);

    const oversizedInput = structuredClone(call);
    for (let index = 0; index < 80; index += 1) oversizedInput.input[`field${index}`] = index;
    expect(validate(oversizedInput)).toBe(false);
  });
});
