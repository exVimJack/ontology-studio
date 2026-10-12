// 从 Turtle 文本提取 owl:Ontology 头部内嵌的宪章（W3C 原生词汇约定）。
// 约定见 crates/ontology-store/src/ttl.rs TtlCharter 注释：
//   charter 四字段可同步以 dcterms: + skos:note 写入 .ttl 文本，本表是可查询的结构化索引。
// 当 ontology_ttl_charter 表为空时，前端回退解析 TTL 头部，保证已内嵌宪章的本体直接可见。

import type { TtlCharter } from "@/lib/domain";

/** 宪章文本字段（不含 updated_by/updated_at 元数据）。 */
type CharterTextField =
  | "business_scenario"
  | "business_essence"


/** 宪章字段 ↔ TTL 头部 W3C 原生词汇映射：业务场景 → dcterms:description；业务本质 → skos:definition。 */
const CHARTER_PREDICATE_TO_FIELD: Record<string, CharterTextField> = {
  // 前缀快捷键写法
  "dcterms:description": "business_scenario",
  "skos:definition": "business_essence",
  // 全 IRI 写法兜底（尖括号格式）
  "http://purl.org/dc/terms/description": "business_scenario",
  "http://www.w3.org/2004/02/skos/core#definition": "business_essence",
};

/** 从 Turtle 文本提取 owl:Ontology 头部内嵌的宪章字段。
 *  @returns 非空对象（可能只匹配到部分字段），或 null（未找到 owl:Ontology 声明或未匹配到任何字段）。
 *  **注意**：仅限 owl:Ontology 声明块内的同谓词，不会误取类/属性上的。 */
export function extractTtlCharterFromContent(
  ttl: string,
): Partial<TtlCharter> | null {
  // 1. 定位 owl:Ontology 声明
  const ontoIdx = ttl.search(/a\s+owl:Ontology\b/);
  if (ontoIdx < 0) return null;

  // 2. 取声明所在行开头（含可能的 subject）
  const blockStart = ttl.lastIndexOf("\n", ontoIdx) + 1;

  // 3. 扫描块结束符（第一个不在引号内的 `.`）
  let inStr = false;
  let esc = false;
  let blockEnd = -1;
  for (let i = ontoIdx; i < ttl.length; i++) {
    const ch = ttl[i];
    if (inStr) {
      if (esc) {
        esc = false;
        continue;
      }
      if (ch === "\\") {
        esc = true;
        continue;
      }
      if (ch === '"') inStr = false;
      continue;
    }
    if (ch === '"') {
      inStr = true;
      continue;
    }
    if (ch === ".") {
      blockEnd = i;
      break;
    }
  }
  if (blockEnd < 0) blockEnd = ttl.length;

  // 4. 在块内提取宪章谓词值对
  const block = ttl.slice(blockStart, blockEnd);
  const result: Partial<TtlCharter> = {};
  // 谓词可能是 `prefix:name` 或 `<full-iri>`，值是被双引号包裹的字符串
  const pairRe = /([^\s;]+)\s+("(?:[^"\\]|\\.)*")/g;
  let m: RegExpExecArray | null;
  while ((m = pairRe.exec(block)) !== null) {
    let pred = m[1];
    // 全 IRI 写法：去掉尖括号
    if (pred.startsWith("<") && pred.endsWith(">")) {
      pred = pred.slice(1, -1);
    }
    const field = CHARTER_PREDICATE_TO_FIELD[pred];
    if (!field) continue;
    // 去掉引号 + 转义还原
    const value = m[2].slice(1, -1).replace(/\\"/g, '"').replace(/\\\\/g, "\\");
    if (!result[field]) result[field] = value;
  }

  return Object.keys(result).length > 0 ? result : null;
}

/** 判断宪章对象是否有实质内容（非空字段）。 */
export function hasCharterContent(c: TtlCharter): boolean {
  return !!(
    c.business_scenario || c.business_essence
  );
}