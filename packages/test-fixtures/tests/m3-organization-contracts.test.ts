import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { load } from "js-yaml";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import { m3Fixtures } from "@mediaflow/test-fixtures";

const root = new URL("../../../", import.meta.url);
const openApi = load(readFileSync(new URL("contracts/openapi/mediaflow.v1.yaml", root), "utf8")) as {
  paths: Record<string, Record<string, unknown>>;
  components: { schemas: Record<string, unknown> };
};
const apiAjv = new Ajv2020({ allErrors: true, strict: false });
addFormats(apiAjv);
apiAjv.addSchema({ ...openApi, $id: "https://mediaflow.local/contracts/openapi/mediaflow.v1.yaml" });

describe("M3 safe organization contract", () => {
  it("defines targets and versioned processing-task organization commands", () => {
    expect(Object.keys(openApi.paths["/api/v1/organization-targets"] ?? {})).toEqual(["get", "post"]);
    expect(Object.keys(openApi.paths["/api/v1/organization-targets/preflights"] ?? {})).toEqual(["post"]);
    expect(Object.keys(openApi.paths["/api/v1/organization-targets/{organizationTargetId}"] ?? {})).toEqual(["parameters", "get", "put", "delete"]);
    expect(Object.keys(openApi.paths["/api/v1/processing-tasks/{processingTaskId}/organization"] ?? {})).toEqual(["parameters", "get"]);
    for (const command of ["recalculations", "executions", "rollbacks"]) {
      expect(Object.keys(openApi.paths[`/api/v1/processing-tasks/{processingTaskId}/organization/${command}`] ?? {})).toEqual(["parameters", "post"]);
    }

    for (const name of [
      "OrganizationTargetPage",
      "OrganizationTarget",
      "OrganizationTargetInput",
      "OrganizationTargetPreflightRequest",
      "OrganizationTargetPreflight",
      "ProcessingTaskOrganization",
      "OrganizationCommandVersion",
    ]) {
      expect(openApi.components.schemas[name], name).toBeDefined();
    }
  });

  it("keeps target and task projections host-path and destructive-command free", () => {
    const serialized = JSON.stringify([
      openApi.components.schemas.OrganizationTarget,
      openApi.components.schemas.ProcessingTaskOrganization,
    ]).toLowerCase();
    for (const forbidden of [
      "absolute_path", "container_path", "host_path", "raw_path", "nfo_xml",
      "file_content", "force", "overwrite", "delete_data",
    ]) {
      expect(serialized).not.toContain(forbidden);
    }
  });

  it("keeps the shared ProcessingTask lifecycle closed over every organization state", () => {
    const processing = openApi.components.schemas.ProcessingTask as {
      properties: {
        status: { enum: string[] };
        checkpoint: { enum: string[] };
      };
    };
    const reasons = openApi.components.schemas.StableReason as { enum: string[] };
    expect(processing.properties.status.enum).toEqual(expect.arrayContaining([
      "partial-success", "completed", "failed",
    ]));
    expect(processing.properties.checkpoint.enum).toEqual(expect.arrayContaining([
      "planning-requested", "plan-prepared", "execution-authorized", "planning-paused",
      "file-operation-prepared", "file-operation-executing", "file-operation-verified",
      "file-operation-manual-review", "nfo-pending", "nfo-verified", "nfo-failed",
      "local-result-prepared", "catalog-committed",
    ]));
    expect(reasons.enum).toEqual(expect.arrayContaining([
      "organization.plan-paused", "organization.io-temporary", "organization.manual-review",
      "organization.nfo-failed", "organization.catalog-unavailable",
    ]));
  });

  it("exports bounded examples that validate against their schemas", () => {
    for (const [schemaName, fixture] of [
      ["OrganizationTargetPage", m3Fixtures.organizationTargetPage],
      ["OrganizationTarget", m3Fixtures.organizationTarget],
      ["OrganizationTargetPreflight", m3Fixtures.organizationTargetPreflight],
      ["ProcessingTaskOrganization", m3Fixtures.processingTaskOrganization],
    ] as const) {
      const validator = apiAjv.getSchema(`https://mediaflow.local/contracts/openapi/mediaflow.v1.yaml#/components/schemas/${schemaName}`);
      expect(validator, schemaName).toBeDefined();
      expect(validator?.(fixture), `${schemaName}: ${JSON.stringify(validator?.errors)}`).toBe(true);
    }

    const serialized = JSON.stringify(m3Fixtures).toLowerCase();
    for (const forbidden of ["container_path", "absolute_path", "nfo_xml", "file_content", "overwrite"]) {
      expect(serialized).not.toContain(forbidden);
    }
  });
});
