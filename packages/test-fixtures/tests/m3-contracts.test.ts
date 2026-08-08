import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import Ajv2020, { type AnySchema } from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import { load } from "js-yaml";
import { m2Fixtures, m3Fixtures as publicFixtures } from "@mediaflow/test-fixtures";
import {
  invalidM3EventSecretLeak,
  invalidM3ResponseSecretLeak,
  validM3EventExamples,
  validM3RestContractExamples,
} from "../src/m3/index.js";

const root = new URL("../../../", import.meta.url);
const openApi = load(readFileSync(new URL("contracts/openapi/mediaflow.v1.yaml", root), "utf8")) as {
  paths: Record<string, Record<string, unknown>>;
  components: { schemas: Record<string, unknown> };
};
const apiAjv = new Ajv2020({ allErrors: true, strict: false });
addFormats(apiAjv);
apiAjv.addSchema({ ...openApi, $id: "https://mediaflow.local/contracts/openapi/mediaflow.v1.yaml" });
const eventAjv = new Ajv2020({ allErrors: true, strict: false });
addFormats(eventAjv);
const eventSchema = JSON.parse(readFileSync(new URL("contracts/events/v1/task-event.schema.json", root), "utf8")) as AnySchema;
const eventValidator = eventAjv.compile(eventSchema);

describe("M3 identification contract fixtures", () => {
  it("accepts every bounded REST and SSE example", () => {
    for (const [schemaName, fixture] of validM3RestContractExamples()) {
      const validator = apiAjv.getSchema(`https://mediaflow.local/contracts/openapi/mediaflow.v1.yaml#/components/schemas/${schemaName}`);
      expect(validator, schemaName).toBeDefined();
      expect(validator?.(fixture), `${schemaName}: ${JSON.stringify(validator?.errors)}`).toBe(true);
    }
    for (const event of [...validM3EventExamples(), m2Fixtures.progress, m2Fixtures.stateChanged, m2Fixtures.gap]) {
      expect(eventValidator(event), JSON.stringify(eventValidator.errors)).toBe(true);
    }
  });

  it("rejects credential leaks from responses and events", () => {
    const integration = apiAjv.getSchema("https://mediaflow.local/contracts/openapi/mediaflow.v1.yaml#/components/schemas/TmdbIntegration");
    expect(integration?.(invalidM3ResponseSecretLeak())).toBe(false);
    expect(eventValidator(invalidM3EventSecretLeak())).toBe(false);
    expect(JSON.stringify([publicFixtures.tmdbIntegration, ...validM3EventExamples()]).toLowerCase()).not.toContain("token");
  });

  it("marks every credential input write-only and keeps all result collections bounded", () => {
    for (const name of ["PutTmdbIntegrationRequest", "TmdbConnectionTestRequest"]) {
      const schema = openApi.components.schemas[name] as { properties: { api_read_access_token: { writeOnly?: boolean } } };
      expect(schema.properties.api_read_access_token.writeOnly, name).toBe(true);
    }
    for (const [name, property, maximum] of [
      ["ProcessingTaskPage", "items", 200],
      ["ReviewCasePage", "items", 200],
      ["IdentificationDetail", "evidence", 200],
      ["IdentificationDetail", "candidates", 100],
    ] as const) {
      const schema = openApi.components.schemas[name] as { properties: Record<string, { maxItems?: number }> };
      expect(schema.properties[property].maxItems, `${name}.${property}`).toBe(maximum);
    }
  });

  it("uses closed enums for processing, decision, review, and event reason codes", () => {
    const stableReason = openApi.components.schemas.StableReason as { enum?: string[] };
    const evidenceReason = openApi.components.schemas.EvidenceReason as { enum?: string[] };
    expect(stableReason.enum?.length).toBeGreaterThan(10);
    expect(evidenceReason.enum).toContain("filename.title");
    expect(stableReason.enum).toContain(publicFixtures.reviewCase.reason);
    expect(stableReason.enum).toContain(publicFixtures.processingStateChanged.payload.reason);
    expect(eventValidator({
      ...publicFixtures.identificationDecided,
      payload: { ...publicFixtures.identificationDecided.payload, reason: "arbitrary.future.reason" },
    })).toBe(false);
    expect(eventValidator({
      ...publicFixtures.integrationHealthChanged,
      payload: { ...publicFixtures.integrationHealthChanged.payload, failure_code: "raw.upstream.message" },
    })).toBe(false);
  });

  it("exposes review decisions and formal catalog reads without file-operation APIs", () => {
    expect(Object.keys(openApi.paths["/api/v1/review-cases"] ?? {})).toEqual(["get"]);
    expect(Object.keys(openApi.paths["/api/v1/review-cases/{reviewCaseId}"] ?? {})).toEqual(["parameters", "get"]);
    expect(openApi.paths["/api/v1/review-cases/{reviewCaseId}/candidates"]).toEqual(expect.objectContaining({ get: expect.any(Object) }));
    expect(openApi.paths["/api/v1/review-cases/{reviewCaseId}/decisions"]).toEqual(expect.objectContaining({ post: expect.any(Object) }));
    expect(openApi.paths["/api/v1/media-items"]).toEqual(expect.objectContaining({ get: expect.any(Object) }));
    expect(openApi.paths["/api/v1/media-items/{mediaItemId}"]).toEqual(expect.objectContaining({ get: expect.any(Object) }));
    expect(openApi.paths).not.toHaveProperty("/api/v1/file-operations");
  });

  it("bounds review candidate, task-center, and catalog result collections", () => {
    for (const [name, property, maximum] of [
      ["ReviewCandidatePage", "items", 20],
      ["ProcessingTaskPage", "items", 200],
      ["MediaItemPage", "items", 200],
    ] as const) {
      const schema = openApi.components.schemas[name] as { properties?: Record<string, { maxItems?: number }> } | undefined;
      expect(schema, name).toBeDefined();
      expect(schema?.properties?.[property]?.maxItems, `${name}.${property}`).toBe(maximum);
    }
  });

  it("accepts only minimal review and catalog refresh events", () => {
    const accepted = {
      id: 45,
      type: "task-decision.accepted",
      schema_version: "1",
      occurred_at: "2026-07-23T09:00:00Z",
      task_id: "019f0000-0000-7000-8000-000000000043",
      payload: {
        case_id: "019f0000-0000-7000-8000-000000000048",
        decision_id: "019f0000-0000-7000-8000-000000000049",
        kind: "select-provider-candidate",
        case_version: 2,
      },
    };
    expect(eventValidator(accepted), JSON.stringify(eventValidator.errors)).toBe(true);
    expect(eventValidator({ ...accepted, payload: { ...accepted.payload, title: "must-not-leak" } })).toBe(false);
  });
});
