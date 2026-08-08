/**
 * 将原型 location hash 解析为页面、可选资源标识和查询参数。
 *
 * @param {string} hash - 例如 `#/task/task-review?view=actionable` 的 hash 文本。
 * @returns {{ page: string, id: string | null, params: Record<string, string> }} 规范化路由；空 hash 选择 `overview`。
 */
export function parseHash(hash) {
  const raw = hash.replace(/^#\/?/, '');
  if (!raw) return { page: 'overview', id: null, params: {} };
  const [path, query = ''] = raw.split('?');
  const [page, id = null] = path.split('/');
  return { page, id, params: Object.fromEntries(new URLSearchParams(query)) };
}

/**
 * 在不改变浏览器状态的前提下，为原型页面构建编码后的 hash URL。
 *
 * @param {string} page - 放在 `#/` 后的原型页面名称。
 * @param {{ id?: string | null, [name: string]: unknown }} [options={}] - 可选路径标识及 `URLSearchParams` 接受的查询值。
 * @returns {string} 对标识和非标识值进行 URL 编码后的 hash URL。
 */
export function hrefFor(page, options = {}) {
  const { id = null, ...params } = options;
  const query = new URLSearchParams(params).toString();
  return `#/${page}${id ? `/${encodeURIComponent(id)}` : ''}${query ? `?${query}` : ''}`;
}
