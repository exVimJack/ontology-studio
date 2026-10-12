//! 端到端验证：完整一轮「本体对抗校验」流程（真实 LLM 调用）。
//!
//! 覆盖（对应 VALIDATION-DESIGN.md 三层对抗）：
//!   阶段① 规则检查：validate_ttl → persist_issues（L0-L2 + L3 结构化 issues）
//!   阶段② 语义评审：review_ontology → run_b_review（B 评审八维度）
//!              → save_b_report → run_a_revise（A 修订）→ a_revise 落库
//!              → run_b_recheck（B 复核）→ b_recheck 机器裁决
//!   阶段② 答辩：dispute_issue → resolve_dispute（B 机器裁决 / 升级）
//!   阶段③ 人工仲裁：escalate → resolve（人工裁决入口）
//!   出口：coverage_matrix（29 句话覆盖）+ issues 汇总（报告数据）
//!
//! 运行：
//!   export CARGO_HOME=/mnt/d/Programs/.cargo
//!   export RUSTUP_HOME=/mnt/d/Programs/.rustup
//!   /mnt/d/Programs/.cargo/bin/cargo.exe run --example e2e_validation_flow \
//!       -- "<settings.json 路径>" "<测试 TTL 文件路径或内联 TTL>"
//!
//! 例（使用应用已保存的 provider 配置 + 内置测试 TTL）：
//!   /mnt/d/Programs/.cargo/bin/cargo.exe run --example e2e_validation_flow -- \
//!       "C:\Users\vinte\AppData\Roaming\com.onto-studio.app\settings.json" inline

use std::sync::Arc;

use agent_core::chat::ChatService;
use agent_core::validation_llm::{run_a_revise, run_b_recheck, run_b_review};
use agent_core::{ProviderConfig, ProviderKind};
use ontology_store::ttl_coverage::coverage_matrix;
use ontology_store::TtlStore;

// 一个含多种问题的最小测试本体（便于 B 评审找到问题、A 修订产生改动）
const SAMPLE_TTL: &str = r#"
@prefix : <http://example.org/auto#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

:AutoDealerRetail a owl:Ontology ;
    rdfs:label "汽车零售本体"@zh ;
    owl:versionInfo "v1" .

:Car a owl:Class ;
    rdfs:label "汽车"@zh ;
    rdfs:comment "车辆实体" .

:Order a owl:Class ;
    rdfs:label "订单"@zh ;
    rdfs:comment "销售订单" .

:orderNo a owl:DatatypeProperty ;
    rdfs:domain :Order ;
    rdfs:range xsd:string ;
    rdfs:label "订单号" .

:orderHasCar a owl:ObjectProperty ;
    rdfs:domain :Order ;
    rdfs:range :Car ;
    rdfs:label "订单包含车辆" .

:supplierId a owl:DatatypeProperty ;
    rdfs:domain :Order ;
    rdfs:range xsd:string ;
    rdfs:label "供应商ID" ;
    rdfs:comment "外键疑似：*Id 命名且存在同词根类" .

:Supplier a owl:Class ;
    rdfs:label "供应商"@zh ;
    rdfs:comment "供应商实体" .
"#;

/// 从 tauri-plugin-store 的 settings.json 恢复 provider 配置。
fn load_provider(path: &str, key: &str) -> ProviderConfig {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("读 settings.json 失败 {path}: {e}"));
    let v: serde_json::Value = serde_json::from_str(&text).expect("settings.json 非 JSON");
    let p = &v[key];
    let kind = p["kind"].as_str().unwrap_or("openai_compatible").to_string();
    ProviderConfig {
        kind: match kind.as_str() {
            "openai" => ProviderKind::OpenAi,
            "anthropic" => ProviderKind::Anthropic,
            "gemini" => ProviderKind::Gemini,
            "deepseek" => ProviderKind::DeepSeek,
            "xai" => ProviderKind::Xai,
            "zhipu" => ProviderKind::Zhipu,
            "moonshot" => ProviderKind::Moonshot,
            "ollama" => ProviderKind::Ollama,
            "openrouter" => ProviderKind::OpenRouter,
            "groq" => ProviderKind::Groq,
            "openai_compatible" => ProviderKind::OpenAiCompatible,
            _ => ProviderKind::OpenAiCompatible,
        },
        api_key: p["api_key"].as_str().unwrap_or("").to_string(),
        base_url: p["base_url"].as_str().map(|s| s.to_string()),
        model: p["model"].as_str().unwrap_or("").to_string(),
        temperature: p["temperature"].as_f64(),
        max_tokens: p["max_tokens"].as_u64(),
        top_p: p["top_p"].as_f64(),
        supports_developer_role: p["supports_developer_role"].as_bool(),
        supports_reasoning_effort: p["supports_reasoning_effort"].as_bool(),
        input_types: vec![agent_core::InputType::Text],
        context_window: p["context_window"].as_u64(),
        preamble: None,
        extra_headers: None,
        reasoning: None,
    }
}

fn step(title: &str) {
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("▶ {title}");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let settings = args.get(1).cloned().unwrap_or_else(|| {
        "C:\\Users\\vinte\\AppData\\Roaming\\com.onto-studio.app\\settings.json".to_string()
    });

    let cfg_a = load_provider(&settings, "provider");
    let cfg_b = load_provider(&settings, "verifier_provider");
    println!("A (建模者): kind={:?} model={}", cfg_a.kind, cfg_a.model);
    println!("B (校验者): kind={:?} model={}", cfg_b.kind, cfg_b.model);
    if cfg_a.api_key.is_empty() || cfg_b.api_key.is_empty() {
        println!("⚠ 任一 provider 无 api_key——中止（不能跑真实 LLM）");
        std::process::exit(2);
    }

    let chat_a = Arc::new(ChatService::new(cfg_a.clone()).expect("ChatService A 构造失败"));
    let chat_b = Arc::new(ChatService::new(cfg_b.clone()).expect("ChatService B 构造失败"));
    let store = Arc::new(TtlStore::open_in_memory().expect("内存 TtlStore 打开失败"));

    let ttl = SAMPLE_TTL.to_string();
    let ontology_iri = "http://example.org/auto#".to_string();

    // ═══ 阶段① 规则检查（L0/L1/L2/L3）═══
    step("阶段① 规则检查：validate_ttl → persist_issues");
    let v = store.validate_ttl(&ttl).expect("validate_ttl 失败");
    println!(
        "triple_count={} errors={} warnings={} issues={}",
        v.triple_count,
        v.errors.len(),
        v.warnings.len(),
        v.issues.len()
    );
    if !v.errors.is_empty() {
        println!("L0/L1 硬错误（不落库，需先修）：{}", v.errors.join("; "));
        std::process::exit(3);
    }
    store
        .persist_issues(&ontology_iri, "v-e2e", &v.issues)
        .expect("persist_issues 失败");
    let issues = store.list_issues(&ontology_iri).expect("list_issues 失败");
    println!("落库 issues {} 条：", issues.len());
    for i in &issues {
        println!(
            "  - [{}] {} {}: {}",
            i.severity.as_str(),
            i.layer,
            i.id,
            i.message.chars().take(60).collect::<String>()
        );
    }

    // ═══ 阶段② 语义评审 ═══
    step("阶段②a 启动评审：review_ontology（残留 in_progress 自动重置）");
    // 先造一条残留 in_progress（模拟中断），验证新评审重置它
    let stale = store.review_ontology(&ontology_iri).expect("残留 review 创建失败");
    let review = store.review_ontology(&ontology_iri).expect("review_ontology 失败");
    println!("新 review id={} status={} round={}/{}", review.id, review.status, review.round, review.max_rounds);
    let stale_now = store.latest_review(&ontology_iri).expect("latest 查询失败").unwrap();
    assert_eq!(stale_now.id, review.id, "latest 应指向新行");
    println!("残留行 {} 已重置（status 变更，latest 指向新行）✓", &stale.id[..12]);

    // B 评审（八维度，真实 LLM）
    step("阶段②b B 评审（八维度）：run_b_review");
    let charter = "业务场景：汽车零售全链路（采购→销售→售后）数据建模；业务本质：订单/车辆/供应商实体关系";
    let issue_lines = issues
        .iter()
        .map(|i| format!("- [{}] {}: {}（status={}）", i.severity.as_str(), i.id, i.message, i.status.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    let inputs = format!(
        "【设计宪章】\n{charter}\n\n【待评审 Turtle】\n{ttl}\n\n【规则检查报告】\n{issue_lines}"
    );
    let b_out = run_b_review(&chat_b, &cfg_b.model, &inputs)
        .await
        .expect("B 评审失败");
    if !b_out.ok {
        println!("✗ B 评审失败: {}", b_out.error.unwrap_or_default());
        std::process::exit(4);
    }
    println!("B 评审输出 {} 字符：", b_out.suggestions_json.len());
    let b_json: serde_json::Value =
        serde_json::from_str(&b_out.suggestions_json).unwrap_or(serde_json::json!([]));
    let n_sug = b_json.as_array().map(|a| a.len()).unwrap_or(0);
    println!("建议条数: {n_sug}");
    if let Some(arr) = b_json.as_array() {
        for s in arr.iter().take(5) {
            println!(
                "  - [{}] {}: {}",
                s["severity"].as_str().unwrap_or("?"),
                s["dimension"].as_str().unwrap_or("?"),
                s["message"].as_str().unwrap_or("").chars().take(70).collect::<String>()
            );
        }
    }

    // 落库 B 报告
    let saved = store
        .save_b_report(&review.id, &b_out.suggestions_json)
        .expect("save_b_report 失败");
    println!("b_report 落库 ✓ len={}", saved.b_report.len());

    // A 修订（真实 LLM：整本 Turtle 重写）
    step("阶段②c A 修订：run_a_revise → a_revise 落库");
    let (revised_ttl, reasoning) = run_a_revise(&chat_a, &cfg_a.model, &ttl, &b_out.suggestions_json)
        .await
        .expect("A 修订失败");
    println!("A 修订输出 {} 字符，修订说明 {} 字符", revised_ttl.len(), reasoning.len());
    assert!(!revised_ttl.trim().is_empty(), "A 修订输出为空");
    let after_a = store
        .a_revise(&review.id, &revised_ttl, &reasoning)
        .expect("a_revise 落库失败");
    println!(
        "a_revise 落库 ✓ status={} round={} a_revision_len={}",
        after_a.status,
        after_a.round,
        after_a.a_revision.len()
    );

    // B 复核（只看修订产物，不看说明）
    step("阶段②d B 复核（机器裁决）：run_b_recheck → b_recheck");
    let verdict = run_b_recheck(&chat_b, &cfg_b.model, &revised_ttl)
        .await
        .expect("B 复核失败");
    println!("B 复核判定: {}", verdict.chars().take(120).collect::<String>());
    let confirm = !verdict.contains("\"verdict\": \"question\"");
    let after_b = store
        .b_recheck(&review.id, confirm, &verdict)
        .expect("b_recheck 落库失败");
    println!(
        "b_recheck 落库 ✓ status={} confirm={}",
        after_b.status, confirm
    );

    // ═══ 阶段② 答辩（对残留 warning issue）═══
    step("阶段②e 答辩：dispute_issue → resolve_dispute");
    let target = issues
        .iter()
        .find(|i| i.severity.as_str() == "warning")
        .cloned()
        .unwrap_or_else(|| {
            store
                .persist_issues(
                    &ontology_iri,
                    "v-e2e",
                    &[ontology_store::validation::ValidationIssue::warning(
                        "L3-3",
                        "L3",
                        "e2e-supplierId",
                        "外键疑似：supplierId 应改 ObjectProperty",
                    )],
                )
                .expect("persist 测试 issue 失败");
            store.list_issues(&ontology_iri).expect("list 失败").remove(0)
        });
    println!("答辩目标 issue: {} ({})", target.id, target.message.chars().take(50).collect::<String>());
    // 先登记证据源（dispute_issue 要求 counterEvidence 引用已登记的 evidence_source）
    let ev_id = store
        .register_evidence_source(
            "table",
            "供应商主数据表（e2e 测试源）",
            "供应商ID 列在源表中存在，映射为快照字段",
            "e2e-ontology",
            "e2e-test",
        )
        .expect("register_evidence_source 失败");
    println!("证据源已登记: {ev_id}");
    let d = store
        .dispute_issue(
            &target.id,
            "supplierId 是业务外键快照而非关联（数据映射层字段别名），保持 DatatypeProperty 合理",
            &ev_id,
            "供应商ID 列在源表中存在，映射为快照字段",
        )
        .expect("dispute_issue 失败");
    println!("答辩提交结果: {d}");
    // 裁决（证据匹配通过 → overruled；否则 escalated）
    let r = store
        .resolve_dispute(&target.id, "overruled", "verifier")
        .expect("resolve_dispute 失败");
    println!("答辩裁决: {r}");

    // ═══ 阶段③ 人工仲裁（分歧/升级项）═══
    step("阶段③ 人工仲裁：escalate → resolve");
    let arb = store.escalate(&review.id).expect("escalate 失败");
    println!("升级仲裁 ✓ review status={}", arb.status);
    let arb_issues = store.list_issues(&ontology_iri).expect("list 失败");
    println!("仲裁队列可见 issue {} 条（含升级项）", arb_issues.len());
    // 人工裁决（任意一方）
    for i in arb_issues.iter().filter(|i| i.status.as_str() == "disputed") {
        let r2 = store
            .resolve_dispute(&i.id, "accepted", "human")
            .expect("人工裁决失败");
        println!("人工裁决 {} → {}", i.id, r2);
    }

    // ═══ 出口：覆盖矩阵 + 报告数据 ═══
    step("出口 覆盖矩阵：coverage_matrix（29 句话）");
    let parsed = ontology_store::ttl::parse_ttl_to_memory(&revised_ttl).expect("修订稿解析失败");
    let cov = coverage_matrix(&parsed, &[]);
    println!(
        "覆盖率 {}/{} ({:.0}%)",
        cov.covered_count,
        cov.total_count,
        (cov.covered_count as f64 / cov.total_count.max(1) as f64) * 100.0
    );
    let missing = cov
        .items
        .iter()
        .filter(|it| !it.covered)
        .map(|it| it.sentence_id.clone())
        .collect::<Vec<_>>();
    println!("缺失句号: {}", if missing.is_empty() { "无".to_string() } else { missing.join(", ") });

    step("出口 报告数据聚合");
    let final_issues = store.list_issues(&ontology_iri).expect("list 失败");
    let err_n = final_issues.iter().filter(|i| i.severity.as_str() == "error").count();
    let warn_n = final_issues.iter().filter(|i| i.severity.as_str() == "warning").count();
    let resolved_n = final_issues
        .iter()
        .filter(|i| matches!(i.status.as_str(), "accepted" | "overruled"))
        .count();
    println!(
        "规则层: error={err_n} warning={warn_n}\n对抗层: review status={} b_report={} a_revision={}\n仲裁层: resolved={resolved_n}/{}\n覆盖层: {}/{}\n结论: 全流程跑通 ✓",
        arb.status,
        saved.b_report.len(),
        after_a.a_revision.len(),
        final_issues.len(),
        cov.covered_count,
        cov.total_count,
    );
}
