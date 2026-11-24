/** W3C 新建本体：系统前缀 + 业务编码 → 完整 IRI。 */

/** 默认 IRI 前缀（设置未改时使用）。 */
export const DEFAULT_ONTOLOGY_IRI_PREFIX =
  "https://onto-studio.local/ontology/";

/** 业务编码：字母开头，后接字母数字 / `_` / `-`，最长 64。 */
export const ONTOLOGY_CODE_RE = /^[A-Za-z][A-Za-z0-9_-]{0,63}$/;

/** 规范化前缀：去首尾空白；若不以 `/` 或 `#` 结尾则补 `/`。 */
export function normalizeOntologyIriPrefix(raw: string): string {
  const t = raw.trim();
  if (!t) return DEFAULT_ONTOLOGY_IRI_PREFIX;
  if (t.endsWith("/") || t.endsWith("#")) return t;
  return `${t}/`;
}

/** 前缀是否可接受（须为 http(s) URL）。 */
export function isValidOntologyIriPrefix(raw: string): boolean {
  const p = normalizeOntologyIriPrefix(raw);
  return /^https?:\/\/\S+$/i.test(p);
}

/** 拼完整本体 IRI：`{前缀}{编码}#`。 */
export function buildOntologyIri(prefix: string, code: string): string {
  const p = normalizeOntologyIriPrefix(prefix);
  const c = code.trim();
  return `${p}${c}#`;
}

export function isValidOntologyCode(code: string): boolean {
  return ONTOLOGY_CODE_RE.test(code.trim());
}
