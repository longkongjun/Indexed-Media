import { spawn, type ChildProcess } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, utimes, writeFile } from "node:fs/promises";
import { createServer as createNetServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { createServer, type ViteDevServer } from "vite";

const appRoot = resolve(import.meta.dirname, "..");

async function availablePort(): Promise<number> {
  const server = createNetServer();
  await new Promise<void>((done, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", done); });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("No local port allocated");
  await new Promise<void>((done, reject) => server.close((error) => error ? reject(error) : done()));
  return address.port;
}

async function stop(process: ChildProcess): Promise<void> {
  if (process.exitCode !== null || process.signalCode !== null) return;
  const exited = new Promise<void>((done) => process.once("exit", () => done()));
  process.kill("SIGTERM");
  const timeout = setTimeout(() => process.kill("SIGKILL"), 5_000);
  await exited;
  clearTimeout(timeout);
}

/** Only disposable files are prepared; no DB rows, API responses or EventSource frames are seeded. */
export async function startRealCore() {
  const root = await realpath(await mkdtemp(resolve(tmpdir(), "mediaflow-web-core-")));
  const config = resolve(root, "config");
  const media = resolve(root, "incoming");
  const sample = resolve(media, "inbox", "Local.Journey.2026.mkv");
  const sampleBytes = Buffer.from("disposable local journey media bytes; not playable video\n");
  const binary = resolve(root, "mediaflow-core");
  const sourceBinary = process.env.MEDIAFLOW_CORE_BINARY ?? resolve(appRoot, "../core/target/debug/mediaflow-core");
  await mkdir(config, { recursive: true });
  await mkdir(dirname(sample), { recursive: true });
  await mkdir(resolve(root, "library"));
  await mkdir(resolve(root, "web"));
  await copyFile(sourceBinary, binary);
  await writeFile(sample, sampleBytes);
  const old = new Date(Date.now() - 120_000);
  await utimes(sample, old, old);
  await writeFile(resolve(config, "deployment-roots.json"), JSON.stringify({ roots: [
    { id: "incoming", label: "Local journey incoming", container_path: media, access: "read-only" },
    { id: "library", label: "Local journey library", container_path: resolve(root, "library"), access: "read-write" },
  ] }));
  const webDist = process.env.MEDIAFLOW_REAL_WEB_DIST ? await realpath(process.env.MEDIAFLOW_REAL_WEB_DIST) : undefined;
  const corePort = await availablePort();
  const webPort = webDist ? corePort : await availablePort();
  const baseURL = `http://127.0.0.1:${String(webPort)}`;
  const coreURL = `http://127.0.0.1:${String(corePort)}`;
  const bootstrapSecret = "local-web-core-bootstrap-only";
  const password = "local-web-core-disposable-password";
  const logs: string[] = [];
  const env: NodeJS.ProcessEnv = { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("MEDIAFLOW_") && !key.toLowerCase().includes("proxy"))),
    MEDIAFLOW_MODE: "development", MEDIAFLOW_LISTEN: `127.0.0.1:${String(corePort)}`,
    MEDIAFLOW_CONFIG_DIR: config, MEDIAFLOW_PUBLIC_ORIGIN: baseURL,
    MEDIAFLOW_WEB_DIST: webDist ?? resolve(root, "web"), MEDIAFLOW_BOOTSTRAP_SECRET: bootstrapSecret,
  };
  function launch(): ChildProcess {
    const child = spawn(binary, ["serve"], { env, stdio: ["ignore", "pipe", "pipe"] });
    child.stdout.on("data", (chunk: Buffer) => logs.push(chunk.toString()));
    child.stderr.on("data", (chunk: Buffer) => logs.push(chunk.toString()));
    return child;
  }
  let core = launch();
  let vite: ViteDevServer | undefined;
  try {
    if (!webDist) vite = await createServer({
      root: appRoot,
      server: { host: "127.0.0.1", port: webPort, strictPort: true, proxy: { "/api": { target: coreURL } } },
      logLevel: "error",
    });
  } catch (error) { await stop(core); await rm(root, { recursive: true, force: true }); throw error; }
  async function ready(): Promise<void> {
    for (let attempt = 0; attempt < 100; attempt += 1) {
      if (core.exitCode !== null) throw new Error(`Core exited: ${logs.join("")}`);
      try { if ((await fetch(`${coreURL}/health/ready`)).ok) return; } catch { /* process starting */ }
      await new Promise((done) => setTimeout(done, 100));
    }
    throw new Error(`Core not ready: ${logs.join("")}`);
  }
  try { await ready(); await vite?.listen(); }
  catch (error) { await stop(core); await vite?.close(); await rm(root, { recursive: true, force: true }); throw error; }
  return {
    baseURL, bootstrapSecret, password,
    webTransport: webDist ? "core-static-dist" : "vite-proxy",
    webDistribution: webDist ?? null,
    binarySha256: createHash("sha256").update(await readFile(binary)).digest("hex"),
    async restart() { await stop(core); core = launch(); await ready(); },
    async assertSourceUnchanged() { return (await readFile(sample)).equals(sampleBytes); },
    databaseFacts(taskId: string) {
      const db = new DatabaseSync(resolve(config, "mediaflow.db"), { readOnly: true });
      const key = taskId.replaceAll("-", "").toUpperCase();
      try {
        return {
          quick_check: db.prepare("PRAGMA quick_check").get()?.quick_check,
          task: db.prepare("SELECT status,stage,attempt_count FROM tasks_processing_tasks WHERE hex(id)=?").get(key),
          attempts: db.prepare("SELECT count(*) AS count FROM tasks_processing_attempts WHERE hex(task_id)=?").get(key)?.count,
          scans: db.prepare("SELECT reason,status,count(*) AS count FROM tasks_scan_tasks GROUP BY reason,status").all(),
          manual_scans: db.prepare("SELECT status,count(*) AS count FROM tasks_scan_tasks WHERE reason='manual' GROUP BY status").all(),
          linked_revisions: db.prepare("SELECT count(*) AS count FROM tasks_processing_tasks t JOIN discovery_file_revisions r ON r.id=t.file_revision_id JOIN discovery_tracked_files f ON f.id=t.discovered_file_id AND f.current_revision_id=r.id WHERE hex(t.id)=?").get(key)?.count,
          source_sha256: createHash("sha256").update(readFileSync(sample)).digest("hex"),
        };
      } finally { db.close(); }
    },
    async close() {
      try { await stop(core); } finally {
        try { await vite?.close(); } finally { await rm(root, { recursive: true, force: true }); }
      }
    },
  };
}
