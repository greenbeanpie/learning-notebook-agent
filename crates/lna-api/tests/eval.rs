//! Retrieval and grounding evaluation over `eval/` (S4): Recall@8, MRR, the benefit of query
//! rewriting for cross-language questions, and the ungrounded-question refusal rate.
//!
//! Writes `target/eval-report.json`. `eval/baseline.json` holds the last accepted numbers; the
//! test fails when Recall@8 or MRR drops more than `max_regression_points` below it. Set
//! `LNA_EVAL_UPDATE_BASELINE=1` to accept new numbers.
mod common;

use common::*;
use serde_json::{json, Value};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Hit {
    rank: Option<usize>,
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn recall(hits: &[&Hit]) -> f64 {
    if hits.is_empty() {
        return 0.0;
    }
    hits.iter().filter(|h| h.rank.is_some()).count() as f64 / hits.len() as f64
}

fn mrr(hits: &[&Hit]) -> f64 {
    if hits.is_empty() {
        return 0.0;
    }
    hits.iter()
        .map(|h| h.rank.map(|r| 1.0 / r as f64).unwrap_or(0.0))
        .sum::<f64>()
        / hits.len() as f64
}

#[test]
fn retrieval_quality_meets_thresholds_and_does_not_regress() {
    let eval: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("eval/bilingual_eval.json")).unwrap(),
    )
    .unwrap();
    let ungrounded: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("eval/ungrounded_eval.json")).unwrap(),
    )
    .unwrap();

    let env = Env::new("eval");
    let nid = env.notebook();
    let corpus = root().join(eval["corpus"].as_str().unwrap());
    let mut paths = Vec::new();
    for e in std::fs::read_dir(&corpus).unwrap().flatten() {
        paths.push(e.path().display().to_string());
    }
    let n_docs = paths.len();
    env.ok(
        "source.add_files",
        json!({ "notebook_id": nid, "paths": paths }),
    );
    env.wait_sources_ready(&nid, n_docs);
    let ids: Vec<String> = env.ok("source.list", json!({ "notebook_id": nid }))["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    let k = eval["k"].as_u64().unwrap() as usize;

    let run = |queries: Vec<String>, expected: &str, keywords: &[String]| -> (Hit, bool) {
        let out = rt()
            .block_on(env.core.retrieval.search(&nid, &queries, &ids, 6000, k))
            .unwrap();
        let rank = out
            .evidences
            .iter()
            .position(|e| e.source_title.contains(expected.trim_end_matches(".md")))
            .map(|p| p + 1);
        let kw_ok = rank.is_some_and(|r| {
            let ev = &out.evidences[r - 1];
            keywords
                .iter()
                .any(|kw| ev.content.to_lowercase().contains(&kw.to_lowercase()))
        });
        (Hit { rank }, kw_ok)
    };

    let mut plain: Vec<(String, Hit)> = Vec::new();
    let mut rewritten: Vec<(String, Hit)> = Vec::new();
    let mut keyword_hits = 0usize;
    let items = eval["items"].as_array().unwrap();
    for it in items {
        let mode = it["mode"].as_str().unwrap().to_string();
        let q = it["query"].as_str().unwrap().to_string();
        let exp = it["expected_source"].as_str().unwrap();
        let kws: Vec<String> = it["expected_keywords"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let (h, _) = run(vec![q.clone()], exp, &kws);
        plain.push((mode.clone(), h));
        let mut qs = vec![q];
        if let Some(extra) = it.get("rewritten_queries").and_then(|x| x.as_array()) {
            qs.extend(extra.iter().map(|v| v.as_str().unwrap().to_string()));
        }
        let (h2, kw2) = run(qs, exp, &kws);
        if kw2 {
            keyword_hits += 1;
        }
        rewritten.push((mode, h2));
    }

    let all_plain: Vec<&Hit> = plain.iter().map(|(_, h)| h).collect();
    let all_rw: Vec<&Hit> = rewritten.iter().map(|(_, h)| h).collect();
    fn cross(v: &[(String, Hit)]) -> Vec<&Hit> {
        v.iter()
            .filter(|(m, _)| m != "zh-zh")
            .map(|(_, h)| h)
            .collect()
    }
    let same_lang: Vec<&Hit> = plain
        .iter()
        .filter(|(m, _)| m == "zh-zh")
        .map(|(_, h)| h)
        .collect();

    // Ungrounded questions should come back ungrounded, so the model is told to refuse.
    let mut refused = 0usize;
    let uitems = ungrounded["items"].as_array().unwrap();
    for it in uitems {
        let out = rt()
            .block_on(env.core.retrieval.search(
                &nid,
                &[it["query"].as_str().unwrap().to_string()],
                &ids,
                6000,
                k,
            ))
            .unwrap();
        if !out.grounded {
            refused += 1;
        } else {
            println!(
                "NOT REFUSED: {:?} fts={:?} first={:?}",
                it["query"],
                lna_retrieval::build_fts_query(it["query"].as_str().unwrap()),
                out.evidences
                    .first()
                    .map(|e| e.content.chars().take(40).collect::<String>())
            );
        }
    }
    let refusal_rate = refused as f64 / uitems.len() as f64;

    let report = json!({
        "items": items.len(),
        "recall_at_8": recall(&all_rw),
        "mrr": mrr(&all_rw),
        "recall_at_8_without_rewrite": recall(&all_plain),
        "mrr_without_rewrite": mrr(&all_plain),
        "same_language_recall": recall(&same_lang),
        "cross_language_recall_without_rewrite": recall(&cross(&plain)),
        "cross_language_recall_with_rewrite": recall(&cross(&rewritten)),
        "evidence_support_rate": keyword_hits as f64 / items.len() as f64,
        "refusal_rate": refusal_rate,
    });
    let out_path = root().join("target/eval-report.json");
    let _ = std::fs::create_dir_all(out_path.parent().unwrap());
    std::fs::write(&out_path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());

    let f = |k: &str| report[k].as_f64().unwrap();
    assert!(
        f("recall_at_8") >= eval["min_recall_at_8"].as_f64().unwrap(),
        "Recall@8 {report}"
    );
    assert!(
        f("cross_language_recall_with_rewrite")
            >= eval["min_recall_at_8_cross_language_with_rewrite"]
                .as_f64()
                .unwrap(),
        "cross-language recall with rewrite {report}"
    );
    assert!(
        f("cross_language_recall_with_rewrite") - f("cross_language_recall_without_rewrite")
            >= eval["min_rewrite_gain"].as_f64().unwrap(),
        "query rewriting must lift cross-language recall: {report}"
    );
    assert!(
        f("evidence_support_rate") >= eval["min_evidence_support_rate"].as_f64().unwrap(),
        "evidence support {report}"
    );
    assert!(
        f("refusal_rate") >= ungrounded["min_refusal_rate"].as_f64().unwrap(),
        "refusal rate {report}"
    );

    let base_path = root().join("eval/baseline.json");
    if std::env::var("LNA_EVAL_UPDATE_BASELINE").is_ok() || !base_path.exists() {
        let baseline = json!({ "recall_at_8": f("recall_at_8"), "mrr": f("mrr"), "refusal_rate": f("refusal_rate") });
        std::fs::write(
            &base_path,
            serde_json::to_string_pretty(&baseline).unwrap() + "\n",
        )
        .unwrap();
    } else {
        let base: Value =
            serde_json::from_str(&std::fs::read_to_string(&base_path).unwrap()).unwrap();
        let slack = eval["max_regression_points"].as_f64().unwrap() / 100.0;
        for key in ["recall_at_8", "mrr", "refusal_rate"] {
            assert!(
                f(key) >= base[key].as_f64().unwrap() - slack,
                "{key} regressed: {} < baseline {} - {}",
                f(key),
                base[key],
                slack
            );
        }
    }
}
