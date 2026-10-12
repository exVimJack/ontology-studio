//! 宪章层：charter 读写 + 骨架创建 + 头部谓词提取（从 ttl/mod.rs 纯移动拆分——逻辑零改动）。

use super::*;

impl TtlStore {
    /// 读取本体设计宪章（不变点）。
    ///
    /// 无 charter 行时返回空结构体（各字段空串）——业务上视为「尚未定义宪章」。
    /// 不报错，调用方决定是否提示建模者补充。
    pub fn get_charter(&self, ontology_iri: &str) -> StoreResult<TtlCharter> {
        let conn = self.conn.lock().unwrap();
        let row = conn.query_row(
            "SELECT business_scenario, business_essence, updated_by, updated_at \
             FROM ontology_ttl_charter WHERE ontology_iri=?1",
            rusqlite::params![ontology_iri],
            |r| {
                Ok(TtlCharter {
                    business_scenario: r.get(0)?,
                    business_essence: r.get(1)?,
                    updated_by: r.get(2)?,
                    updated_at: r.get(3)?,
                })
            },
        );
        match row {
            Ok(c) => Ok(c),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(TtlCharter::default()),
            Err(e) => Err(e.into()),
        }
    }
}


impl TtlStore {
    /// 写入/更新本体设计宪章（不变点）。
    ///
    /// **charter 不随历史变化**——它记录本体的业务本质说明，不随实体增删改而变更。
    /// 只有用户明确要求调整 charter 时才调用；常规增量更新不应触碰 charter。
    /// 冷启动建模时，如历史对话已提及业务场景/本质/意图，应主动提取后调用本工具落库。
    /// import_ttl 不覆盖 charter（写入路径分离）。
    pub fn set_charter(&self, ontology_iri: &str, charter: &TtlCharter) -> StoreResult<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms();
        conn.execute(
            "INSERT INTO ontology_ttl_charter (ontology_iri, business_scenario, business_essence, updated_by, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(ontology_iri) DO UPDATE SET
                business_scenario = excluded.business_scenario,
                business_essence   = excluded.business_essence,
                updated_by         = excluded.updated_by,
                updated_at         = excluded.updated_at",
            rusqlite::params![
                ontology_iri,
                charter.business_scenario,
                charter.business_essence,
                if charter.updated_by.is_empty() { "agent" } else { &charter.updated_by },
                now,
            ],
        )?;
        drop(conn);
        self.notify_change();
        Ok(())
    }
}


impl TtlStore {
    /// 创建空骨架本体 + 宪章（宪章先行，见 ARCHITECTURE.md 决策 24）。
    ///
    /// **仅前端宪章向导 IPC 可达**——agent 工具层无此路径，本体创建 = 用户
    /// 确认宪章，AI 不得绕过（import 闸门会拒绝不存在的 IRI）。
    /// 骨架 TTL 含 owl:Ontology 头部 + 宪章四谓词（双载体一致），
    /// 宪章行同步落库（表为准，头部为 W3C 原生载体）。
    pub fn create_with_charter(
        &self,
        ontology_iri: &str,
        display_name: &str,
        charter: &TtlCharter,
    ) -> StoreResult<()> {
        if self.exists(ontology_iri)? {
            return Err(StoreError::Other(anyhow::anyhow!(
                "本体已存在: {ontology_iri}"
            )));
        }
        let ttl = Self::skeleton_ttl(ontology_iri, display_name, charter);
        let r = self.import_ttl(&ttl, true)?;
        if !r.errors.is_empty() {
            return Err(StoreError::Other(anyhow::anyhow!(
                "骨架 TTL 落库失败: {:?}",
                r.errors
            )));
        }
        self.set_charter(ontology_iri, charter)
    }
}


impl TtlStore {
    /// 生成空骨架 TTL：owl:Ontology 头部 + 宪章四谓词（W3C 原生词汇）。
    fn skeleton_ttl(iri: &str, display_name: &str, charter: &TtlCharter) -> String {
        // 三引号内不能出现 " 与 """：统一替换为单引号（宪章为业务描述文本，可接受）
        let clean = |s: &str| s.replace('"', "'");
        format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
             @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
             @prefix dcterms: <http://purl.org/dc/terms/> .\n\
             @prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n\
             \
             <{iri}> a owl:Ontology ;\n\
                 rdfs:label \"{}\" ;\n\
                 dcterms:description \"{}\" ;\n\
                 skos:definition \"{}\" .\n",
            clean(display_name),
            clean(&charter.business_scenario),
            clean(&charter.business_essence),
        )
    }
}


/// 提取 owl:Ontology 声明块内嵌的宪章谓词原文（W3C 原生词汇约定）：
/// 仅扫描 owl:Ontology 声明块（到引号外首个 `.` 为止），不碰类/属性上的同谓词。
/// 用于 agent import 路径的宪章变更守卫（宪章用户主权）：与库内原文逐字段比对。
pub fn extract_charter_predicates(ttl: &str) -> [Option<String>; 2] {
    const PREDICATES: [&str; 2] = ["dcterms:description", "skos:definition"];
    let mut out: [Option<String>; 2] = [None, None];
    let Some(pos) = ttl.find("owl:Ontology") else {
        return out;
    };
    let block_start = ttl[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);

    // 扫描块结束符：第一个不在引号内的 '.'
    let bytes = ttl.as_bytes();
    let (mut in_str, mut esc) = (false, false);
    let mut block_end = ttl.len();
    for (i, &b) in bytes.iter().enumerate().skip(pos) {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'.' => {
                block_end = i;
                break;
            }
            _ => {}
        }
    }
    let block = &ttl[block_start..block_end];

    // 逐个扫描 "literal"，回看开引号前的谓词 token
    let bb = block.as_bytes();
    let (mut in_str, mut esc) = (false, false);
    let mut quote_start = 0usize;
    for k in 0..bb.len() {
        let b = bb[k];
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
                // 回看谓词 token（跳过空白后取连续非分隔符字符）
                let mut e = quote_start;
                while e > 0 && (bb[e - 1] as char).is_whitespace() {
                    e -= 1;
                }
                let mut s = e;
                while s > 0 {
                    let c = bb[s - 1] as char;
                    if c.is_whitespace() || c == ';' || c == '(' {
                        break;
                    }
                    s -= 1;
                }
                let pred = &block[s..e];
                let pred = pred
                    .strip_prefix('<')
                    .and_then(|p| p.strip_suffix('>'))
                    .unwrap_or(pred);
                if let Some(idx) = PREDICATES.iter().position(|p| *p == pred) {
                    if out[idx].is_none() {
                        let raw = &block[quote_start + 1..k];
                        out[idx] =
                            Some(raw.replace("\\\"", "\"").replace("\\\\", "\\"));
                    }
                }
            }
            continue;
        }
        if b == b'"' {
            in_str = true;
            quote_start = k;
        }
    }
    out
}
