import { readdir, readFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";
import path from "node:path";

/**
 * 验证构建后的 Web 发行包只有一个带哈希的业务入口，且 gzip 大小不超过 180 KiB。
 *
 * 路由懒加载后同时列出全部异步 chunk 的 gzip 成本与总和，防止只把代码移出入口制造"预算通过"；
 * 异步成本目前仅登记不设限，预算口径变化须按接入文档另行评审。
 *
 * 发行包根目录取自第一个 CLI 参数。脚本会输出测得的字节数；缺少参数、入口文件缺失或不唯一、输出不可读、
 * 或超出预算时都会抛出错误。
 */
const root = process.argv[2];
if (!root) throw new Error("Usage: check-budget.mjs <vite-dist>");
const assets = path.join(root, "assets");
const names = (await readdir(assets)).filter((name) => name.endsWith(".js"));
const files = names.filter((name) => /^index-[A-Za-z0-9_-]+\.js$/.test(name));
if (files.length !== 1) throw new Error(`Expected exactly one business entry JS, found ${files.length}`);
const bytes = gzipSync(await readFile(path.join(assets, files[0]))).byteLength;
console.log(`MediaFlow business entry gzip: ${bytes} bytes`);
if (bytes > 180 * 1024) throw new Error(`Business entry exceeds 180 KiB budget: ${bytes} bytes`);

const chunks = names.filter((name) => !/^index-[A-Za-z0-9_-]+\.js$/.test(name));
if (chunks.length > 0) {
  const sizes = await Promise.all(chunks.map(async (name) => ({ name, gzip: gzipSync(await readFile(path.join(assets, name))).byteLength })));
  sizes.sort((a, b) => b.gzip - a.gzip);
  for (const chunk of sizes) console.log(`  async chunk ${chunk.name}: ${chunk.gzip} bytes (gzip)`);
  const total = sizes.reduce((sum, chunk) => sum + chunk.gzip, 0);
  console.log(`MediaFlow async chunks: ${sizes.length} files, ${total} bytes gzip total; entry+chunks = ${bytes + total} bytes`);
}
