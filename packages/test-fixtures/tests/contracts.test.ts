import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import Ajv2020, { type AnySchema } from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import { load } from "js-yaml";
import { m2Fixtures as publicFixtures } from "@mediaflow/test-fixtures";
import { invalidIdentitySecretLeak, validContractExamples } from "../src/m2/index.js";

const root = new URL("../../../", import.meta.url);
const loadJson = (path: string) => JSON.parse(readFileSync(new URL(path, root), "utf8")) as unknown;
const isHostAbsolutePath = (value: string) => value.startsWith("/")
  || value.startsWith("\\")
  || /^[a-z]:[\\/]/i.test(value);

function fixturePathFields(value: unknown, key = "recovery"): Array<{ key: string; value: string }> {
  if (Array.isArray(value)) return value.flatMap((item, index) => fixturePathFields(item, `${key}[${index}]`));
  if (!value || typeof value !== "object") return [];
  return Object.entries(value as Record<string, unknown>).flatMap(([childKey, child]) => {
    const qualifiedKey = `${key}.${childKey}`;
    if (typeof child === "string" && (childKey === "path" || childKey.endsWith("_path"))) {
      return [{ key: qualifiedKey, value: child }];
    }
    return fixturePathFields(child, qualifiedKey);
  });
}

function expectSafeRelativeFixturePath(field: { key: string; value: string }): void {
  expect(field.value, field.key).not.toBe("");
  expect(isHostAbsolutePath(field.value), field.key).toBe(false);
  expect(field.value, field.key).not.toMatch(/[\\\u0000-\u001f\u007f]/);
  expect(field.value.split("/"), field.key).not.toContain("");
  expect(field.value.split("/"), field.key).not.toContain(".");
  expect(field.value.split("/"), field.key).not.toContain("..");
}

const openApi = load(readFileSync(new URL("contracts/openapi/mediaflow.v1.yaml", root), "utf8")) as { components: { schemas: Record<string, unknown> } };
const apiAjv = new Ajv2020({ allErrors: true, strict: false });
addFormats(apiAjv);
apiAjv.addSchema({ ...openApi, $id: "https://mediaflow.local/contracts/openapi/mediaflow.v1.yaml" });
const eventAjv = new Ajv2020({ allErrors: true, strict: false });
addFormats(eventAjv);
const eventSchema = loadJson("contracts/events/v1/task-event.schema.json");
const eventValidator = eventAjv.compile(eventSchema as AnySchema);

describe("M2 contract fixtures", () => {
  it("defines M4 source automation, event, and enhancer fixtures without secret projections", () => {
    const examples = [
      ["AutomationSourcePage", loadJson("contracts/examples/v1/automation-source-page.json")],
      ["AutomationSource", loadJson("contracts/examples/v1/automation-source.json")],
      ["AutomationSourceConnectionTestResult", loadJson("contracts/examples/v1/automation-source-connection-test-result.json")],
      ["WebhookSecretReceipt", loadJson("contracts/examples/v1/automation-webhook-secret-receipt.json")],
      ["AutomationEventPage", loadJson("contracts/examples/v1/automation-event-page.json")],
      ["AutomationEvent", loadJson("contracts/examples/v1/automation-event.json")],
      ["IdentificationEnhancer", loadJson("contracts/examples/v1/identification-enhancer.json")],
      ["IdentificationEnhancerConnectionTestResult", loadJson("contracts/examples/v1/identification-enhancer-connection-test-result.json")],
    ] as const;
    for (const [schemaName, example] of examples) {
      const validator = apiAjv.getSchema(`https://mediaflow.local/contracts/openapi/mediaflow.v1.yaml#/components/schemas/${schemaName}`);
      expect(validator, schemaName).toBeDefined();
      expect(validator?.(example), `${schemaName}: ${JSON.stringify(validator?.errors)}`).toBe(true);
    }
    for (const path of [
      "/api/v1/automation-sources",
      "/api/v1/automation-sources/{automationSourceId}",
      "/api/v1/automation-sources/connection-tests",
      "/api/v1/automation-sources/{automationSourceId}/secret-rotations",
      "/api/v1/automation-events",
      "/api/v1/automation-events/{automationEventId}",
      "/api/v1/automation-events/{automationEventId}/retries",
      "/api/v1/automation-events/{automationEventId}/cancellations",
      "/api/v1/identification-enhancer",
      "/api/v1/identification-enhancer/connection-tests",
      "/api/v1/source-webhooks/{automationSourceId}/events",
    ]) {
      expect((openApi as unknown as { paths: Record<string, unknown> }).paths).toHaveProperty(path);
    }
    const serialized = JSON.stringify(examples).toLowerCase();
    for (const forbidden of ["feed_url", "download_source", "absolute_path", "model_input", "model_output", "signature"]) {
      expect(serialized).not.toContain(forbidden);
    }
  });

  it("accepts every positive example", () => {
    const schemaNames = [
      "BootstrapStatusResponse", "BootstrapRequest", "SessionResponse", "DeploymentRootList",
      "InboxDirectoryPreflight", "InboxDirectory", "ScanTask", "DiscoveredFilePage", "ScanErrorPage",
    ];
    for (const [index, schemaName] of schemaNames.entries()) {
      const validator = apiAjv.getSchema(`https://mediaflow.local/contracts/openapi/mediaflow.v1.yaml#/components/schemas/${schemaName}`);
      expect(validator, schemaName).toBeDefined();
      expect(validator?.(validContractExamples()[index]), `${schemaName}: ${JSON.stringify(validator?.errors)}`).toBe(true);
    }
    for (const event of validContractExamples().slice(9)) {
      expect(eventValidator(event), JSON.stringify(eventValidator.errors)).toBe(true);
    }
  });

  it("rejects the identity response which leaks bootstrap_secret", () => {
    const validator = apiAjv.getSchema("https://mediaflow.local/contracts/openapi/mediaflow.v1.yaml#/components/schemas/SessionResponse");
    expect(validator?.(invalidIdentitySecretLeak())).toBe(false);
    expect(validator?.errors?.some((error: { keyword: string; params: { additionalProperty?: string } }) => error.keyword === "additionalProperties" && error.params.additionalProperty === "bootstrap_secret")).toBe(true);
  });

  it("rejects a top-level SSE secret leak", () => {
    const secretLeak = { id: 4, type: "stream.gap", schema_version: "1", occurred_at: "2026-07-17T00:00:03Z", task_id: null, payload: { minimum_available_id: 4 }, bootstrap_secret: "must-not-be-exposed" };
    expect(eventValidator(secretLeak)).toBe(false);
    expect(eventValidator.errors?.some((error) => error.keyword === "additionalProperties" && error.params.additionalProperty === "bootstrap_secret")).toBe(true);
  });

  it("requires a UUID task id for task events and null only for stream gaps", () => {
    for (const event of [publicFixtures.progress, publicFixtures.stateChanged]) {
      expect(eventValidator(event), JSON.stringify(eventValidator.errors)).toBe(true);
      expect(eventValidator({ ...event, task_id: null })).toBe(false);
    }
    expect(eventValidator(publicFixtures.gap), JSON.stringify(eventValidator.errors)).toBe(true);
    expect(eventValidator({ ...publicFixtures.gap, task_id: publicFixtures.task.id })).toBe(false);
  });

  it("keeps event envelopes bounded, path-free, and declares cursor validation", () => {
    const contract = openApi as unknown as {
      paths: Record<string, { get: { responses: Record<string, unknown>; parameters: Array<{ name?: string; schema?: { maximum?: number } }> } }>;
      components: { schemas: Record<string, unknown> };
    };
    expect(contract.paths["/api/v1/events"].get.responses).toHaveProperty("422");
    const cursor = contract.paths["/api/v1/events"].get.parameters.find((parameter) => parameter.name === "Last-Event-ID");
    expect(cursor?.schema?.maximum).toBe(Number.MAX_SAFE_INTEGER);
    const serialized = JSON.stringify([
      contract.components.schemas.TaskProgressEvent,
      contract.components.schemas.TaskStateChangedEvent,
      publicFixtures.progress,
      publicFixtures.stateChanged,
    ]);
    for (const forbidden of ["absolute_path", "relative_path", "cookie", "token", "diagnostic", "stack", "sql"]) {
      expect(serialized.toLowerCase()).not.toContain(forbidden);
    }
    const unsafe = { ...publicFixtures.progress, id: Number.MAX_SAFE_INTEGER + 1 };
    expect(eventValidator(unsafe)).toBe(false);
  });

  it("defines unpaginated deployment-root listing and distinct create/logout cookie constraints", () => {
    const roots = openApi.components.schemas.DeploymentRootList as { properties?: Record<string, unknown>; required?: string[] };
    expect(roots.required).toContain("next_cursor");
    expect(roots.properties?.next_cursor).toBeDefined();
    const rootOperation = (openApi as unknown as { paths: Record<string, { get: { parameters?: Array<{ $ref?: string }> } }> }).paths["/api/v1/deployment-roots"].get;
    expect(rootOperation.parameters ?? []).not.toContainEqual({ $ref: "#/components/parameters/Cursor" });
    const contract = openApi as unknown as {
      components: { parameters: { Cursor: { schema: { maxLength?: number } } } };
      paths: Record<string, Record<string, { responses: Record<string, unknown> }>>;
    };
    expect(contract.components.parameters.Cursor.schema.maxLength).toBe(512);
    for (const [path, method] of [
      ["/api/v1/deployment-roots", "get"],
      ["/api/v1/inbox-directories/preflight", "post"],
      ["/api/v1/inbox-directories", "get"],
      ["/api/v1/inbox-directories", "post"],
      ["/api/v1/inbox-directories/{inboxDirectoryId}", "get"],
    ]) {
      expect(contract.paths[path][method].responses).toHaveProperty("503");
    }
    expect(contract.paths["/api/v1/inbox-directories"].post.responses).toHaveProperty("404");
    const sessionResponse = (openApi as unknown as { components: { responses: { SessionResponse: { headers: { "Set-Cookie": { schema: { pattern?: string } } } } } } }).components.responses.SessionResponse;
    expect(new RegExp(sessionResponse.headers["Set-Cookie"].schema.pattern!).test("__Host-mediaflow_session=session; Secure; HttpOnly; SameSite=Strict; Path=/")).toBe(true);
    const deleteCookie = (openApi as unknown as { paths: Record<string, { delete: { responses: Record<string, { headers: { "Set-Cookie": { schema: { pattern?: string } } } }> } }> }).paths["/api/v1/session"].delete.responses["204"].headers["Set-Cookie"].schema.pattern;
    expect(new RegExp(deleteCookie!).test("__Host-mediaflow_session=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Secure; HttpOnly; SameSite=Strict; Path=/")).toBe(true);
  });

  it("bounds inbox pages and exposes health check time without container paths", () => {
    const inbox = openApi.components.schemas.InboxDirectory as { required?: string[] };
    const page = openApi.components.schemas.InboxDirectoryPage as {
      properties?: { items?: { maxItems?: number } };
    };
    expect(inbox.required).toContain("last_checked_at");
    expect(page.properties?.items?.maxItems).toBe(200);
    expect(JSON.stringify(validContractExamples().slice(3, 6))).not.toContain("container_path");
  });

  it("declares scan list validation responses and only runtime-reachable scan examples", () => {
    const contract = openApi as unknown as {
      paths: Record<string, { get: { responses: Record<string, unknown> } }>;
    };
    for (const path of [
      "/api/v1/scan-tasks",
      "/api/v1/scan-tasks/{scanTaskId}/files",
      "/api/v1/scan-tasks/{scanTaskId}/errors",
    ]) {
      expect(contract.paths[path].get.responses).toHaveProperty("422");
    }
    expect(publicFixtures.task.counts.skipped_entries).toBeGreaterThan(0);
    expect(publicFixtures.errors.items[0]?.code).toBe("entry.unavailable");
    expect(publicFixtures.errors.items[0]?.code).not.toBe("path.symlink_forbidden");
  });

  it("declares a stable fixture package entrypoint", () => {
    const manifest = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
    expect(manifest.types).toBe("./src/index.ts");
    expect(manifest.exports).toEqual({ ".": { types: "./src/index.ts", default: "./src/index.ts" } });
  });

  it("resolves its public entrypoint by package name", () => {
    expect(publicFixtures.bootstrapStatus.requires_initialization).toBe(true);
  });

  it("exports a deterministic, identity-stable recovery scenario without deployment secrets", () => {
    const recovery = (publicFixtures as typeof publicFixtures & {
      recovery?: {
        taskId: string;
        snapshots: { retained: { id: string }; recovering: { id: string }; terminal: { id: string } };
        files: { items: Array<{ id: string; relative_path: string }> };
        errors: { items: Array<{ id: string; relative_path: string; code: string }> };
      };
    }).recovery;

    expect(recovery).toBeDefined();
    expect(new Set([
      recovery?.taskId,
      recovery?.snapshots.retained.id,
      recovery?.snapshots.recovering.id,
      recovery?.snapshots.terminal.id,
    ])).toEqual(new Set([publicFixtures.task.id]));
    expect(recovery?.files.items).toEqual(publicFixtures.files.items);
    expect(recovery?.errors.items).toEqual(publicFixtures.errors.items);
    const pathFields = fixturePathFields(recovery);
    expect(pathFields.length).toBeGreaterThan(0);
    pathFields.forEach(expectSafeRelativeFixturePath);

    const serialized = JSON.stringify(recovery).toLowerCase();
    for (const forbidden of [
      "bootstrap_secret", "csrf", "token", "cookie", "password", "authorization",
      "stack", "select ", "insert ", "update ", "delete from",
    ]) {
      expect(serialized).not.toContain(forbidden);
    }
  });

  it("rejects every host-absolute path form rather than a short prefix list", () => {
    for (const absolutePath of [
      "/tmp/movie.mkv",
      "/private/var/movie.mkv",
      "D:\\Media\\movie.mkv",
      "E:/Media/movie.mkv",
      "\\\\nas\\incoming\\movie.mkv",
      "//nas/incoming/movie.mkv",
    ]) {
      expect(isHostAbsolutePath(absolutePath), absolutePath).toBe(true);
    }
  });
});
