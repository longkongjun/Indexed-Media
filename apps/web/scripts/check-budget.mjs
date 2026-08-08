import { readdir, readFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";
import path from "node:path";

/**
 * 验证构建后的 Web 发行包只有一个带哈希的业务入口，且 gzip 大小不超过 180 KiB。
 *
 * 发行包根目录取自第一个 CLI 参数。脚本会输出测得的字节数；缺少参数、入口文件缺失或不唯一、输出不可读、
 * 或超出预算时都会抛出错误。
 */
const root = process.argv[2];
if (!root) throw new Error("Usage: check-budget.mjs <vite-dist>");
const assets = path.join(root, "assets");
const files = (await readdir(assets)).filter((name) => /^index-[A-Za-z0-9_-]+\.js$/.test(name));
if (files.length !== 1) throw new Error(`Expected exactly one business entry JS, found ${files.length}`);
const bytes = gzipSync(await readFile(path.join(assets, files[0]))).byteLength;
console.log(`MediaFlow business entry gzip: ${bytes} bytes`);
if (bytes > 180 * 1024) throw new Error(`Business entry exceeds 180 KiB budget: ${bytes} bytes`);
