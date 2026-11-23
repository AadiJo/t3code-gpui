//! Runs the usage merge and limit pooling over scenarios read from a JSON
//! file and prints the results, in the shape the TypeScript runner prints, so
//! the two can be diffed. `cargo run -p t3code-shared --example usage_check -- scenarios.json`

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use t3code_contracts::{EnvironmentId, ServerConfigProvidersItem, ServerConfigUsageLimitSourcesItem, UsageSummary};
use t3code_shared::usage_limits::{LimitEnvironment, collect_limit_accounts, collect_limit_pools, display_limit_windows};
use t3code_shared::usage_merge::{EnvironmentUsage, USAGE_CONTRACT_VERSION, merge_usage, number};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Scenario {
    id: u64,
    now: f64,
    environments: Vec<ScenarioEnvironment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScenarioEnvironment {
    environment_id: EnvironmentId,
    label: String,
    summary: UsageSummary,
    providers: Vec<ServerConfigProvidersItem>,
    usage_limit_sources: Vec<ServerConfigUsageLimitSourcesItem>,
}

fn main() {
    let path = std::env::args().nth(1).expect("scenarios path");
    let scenarios: Vec<Scenario> = serde_json::from_slice(&std::fs::read(path).expect("read")).expect("decode");
    let out: Vec<Value> = scenarios
        .into_iter()
        .map(|scenario| {
            let usage: Vec<EnvironmentUsage> = scenario
                .environments
                .iter()
                .map(|e| EnvironmentUsage {
                    environment_id: e.environment_id.clone(),
                    label: e.label.clone(),
                    summary: Arc::new(e.summary.clone()),
                })
                .collect();
            let merged = merge_usage(&usage, USAGE_CONTRACT_VERSION);
            let limits: Vec<LimitEnvironment> = scenario
                .environments
                .into_iter()
                .map(|e| LimitEnvironment {
                    environment_id: e.environment_id,
                    label: e.label,
                    providers: e.providers.into(),
                    usage_limit_sources: e.usage_limit_sources.into(),
                })
                .collect();
            let accounts = collect_limit_accounts(&limits);
            let accounts_json: Vec<Value> = accounts
                .iter()
                .map(|a| {
                    json!({
                        "key": a.key, "driver": a.driver, "displayName": a.display_name, "email": a.email, "plan": a.plan,
                        "environments": a.environments.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
                        "sourceLabel": a.source_label,
                        "redeem": a.redeem.as_ref().map(|r| json!({"environmentId": r.environment_id, "input": r.input})),
                        "checkedAt": a.limits.checked_at,
                        "credits": a.limits.reset_credits.as_ref().map(|c| c.available_count),
                    })
                })
                .collect();
            let pools = collect_limit_pools(accounts, scenario.now);
            let pools_json: Vec<Value> = pools
                .iter()
                .map(|p| {
                    json!({
                        "driver": p.driver,
                        "accounts": p.accounts.iter().map(|a| a.key.clone()).collect::<Vec<_>>(),
                        "display": display_limit_windows(p).iter().map(|w| w.id.clone()).collect::<Vec<_>>(),
                        "windows": p.windows.iter().map(|w| json!({
                            "id": w.id, "kind": w.kind, "label": w.label, "remaining": w.remaining_percent, "used": w.used_percent,
                            "pace": w.pace,
                            "columns": w.columns.iter().map(|(account, member)| match member {
                                Some(m) => json!([account.key, number(&m.window.used_percent), m.window.resets_at, m.refill_percent]),
                                None => json!([account.key, null]),
                            }).collect::<Vec<_>>(),
                            "resets": w.resets.iter().map(|r| json!([r.member.account.key, r.at, r.restores_percent])).collect::<Vec<_>>(),
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            json!({ "id": scenario.id, "merged": merged, "accounts": accounts_json, "pools": pools_json })
        })
        .collect();
    println!("{}", serde_json::to_string(&out).expect("encode"));
}
