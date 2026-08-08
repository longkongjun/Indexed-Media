import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { load } from "js-yaml";
import Ajv2020, { type AnySchema } from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import { m4Fixtures, validM4EventExamples, validM4RestContractExamples } from "../src/m4/index.js";

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

describe("M4 downloader management contract", () => {
  it("defines the complete connection and download task surface", () => {
    expect(Object.keys(openApi.paths["/api/v1/downloader-connections"] ?? {})).toEqual(["get", "post"]);
    expect(Object.keys(openApi.paths["/api/v1/downloader-connections/{downloaderConnectionId}"] ?? {})).toEqual(["parameters", "get", "put", "delete"]);
    expect(Object.keys(openApi.paths["/api/v1/downloader-connections/connection-tests"] ?? {})).toEqual(["post"]);
    expect(Object.keys(openApi.paths["/api/v1/download-tasks"] ?? {})).toEqual(["get", "post"]);
    expect(Object.keys(openApi.paths["/api/v1/download-tasks/{downloadTaskId}"] ?? {})).toEqual(["parameters", "get"]);

    for (const name of [
      "DownloaderConnection", "DownloaderConnectionPage", "DownloaderConnectionInput",
      "DownloaderConnectionTestResult", "DownloaderCapabilities", "DownloadTask", "DownloadTaskPage",
      "CreateDownloadTaskRequest", "DownloadTaskChangedEvent",
    ]) {
      expect(openApi.components.schemas[name], name).toBeDefined();
    }
  });

  it("marks every downloader secret write-only and keeps public projections bounded", () => {
    const input = openApi.components.schemas.DownloaderConnectionInput as {
      properties?: Record<string, { writeOnly?: boolean }>;
      required?: string[];
    } | undefined;
    const createTask = openApi.components.schemas.CreateDownloadTaskRequest as {
      properties?: Record<string, { writeOnly?: boolean }>;
      required?: string[];
    } | undefined;
    const connectionPage = openApi.components.schemas.DownloaderConnectionPage as {
      properties?: { items?: { maxItems?: number } };
    } | undefined;
    const taskPage = openApi.components.schemas.DownloadTaskPage as {
      properties?: { items?: { maxItems?: number } };
    } | undefined;

    expect(input?.required).toEqual(["kind", "display_name", "base_url", "username", "password", "enabled"]);
    expect(input?.properties?.username?.writeOnly).toBe(true);
    expect(input?.properties?.password?.writeOnly).toBe(true);
    expect(createTask?.required).toEqual(["connection_id", "source", "display_name"]);
    expect(createTask?.properties?.source?.writeOnly).toBe(true);
    expect(connectionPage?.properties?.items?.maxItems).toBe(200);
    expect(taskPage?.properties?.items?.maxItems).toBe(200);
  });

  it("accepts bounded examples and rejects downloader secret leaks", () => {
    for (const [schemaName, fixture] of validM4RestContractExamples()) {
      const validator = apiAjv.getSchema(`https://mediaflow.local/contracts/openapi/mediaflow.v1.yaml#/components/schemas/${schemaName}`);
      expect(validator, schemaName).toBeDefined();
      expect(validator?.(fixture), `${schemaName}: ${JSON.stringify(validator?.errors)}`).toBe(true);
    }
    for (const event of validM4EventExamples()) {
      expect(eventValidator(event), JSON.stringify(eventValidator.errors)).toBe(true);
    }
    const taskValidator = apiAjv.getSchema("https://mediaflow.local/contracts/openapi/mediaflow.v1.yaml#/components/schemas/DownloadTask");
    expect(taskValidator?.({ ...m4Fixtures.downloadTask, source: "magnet:?xt=urn:btih:secret" })).toBe(false);
    expect(eventValidator({
      ...m4Fixtures.downloadTaskChanged,
      payload: { ...m4Fixtures.downloadTaskChanged.payload, password: "secret" },
    })).toBe(false);
  });
});
