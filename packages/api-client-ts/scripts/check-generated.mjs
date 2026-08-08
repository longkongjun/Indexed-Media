import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

/**
 * 在临时目录中重新生成 OpenAPI TypeScript 声明，并逐字节与已提交输出比较。生成错误会向外传播；发现漂移时
 * 会设置非零退出码；无论何种情况都会移除临时目录。此脚本绝不写入已提交的生成声明。
 */
const packageDirectory = fileURLToPath(new URL("..", import.meta.url));
const contract = fileURLToPath(new URL("../../../contracts/openapi/mediaflow.v1.yaml", import.meta.url));
const generated = join(packageDirectory, "src/generated/mediaflow.d.ts");
const temporaryDirectory = mkdtempSync(join(tmpdir(), "mediaflow-openapi-"));
const candidate = join(temporaryDirectory, "mediaflow.d.ts");

try {
  execFileSync("pnpm", ["exec", "openapi-typescript", contract, "-o", candidate], {
    cwd: packageDirectory,
    stdio: "inherit",
  });
  if (!readFileSync(generated).equals(readFileSync(candidate))) {
    console.error("Generated OpenAPI types differ. Run: pnpm --filter @mediaflow/api-client-ts generate");
    process.exitCode = 1;
  }
} finally {
  rmSync(temporaryDirectory, { recursive: true, force: true });
}
