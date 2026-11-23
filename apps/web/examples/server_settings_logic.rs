//! Runs the ported provider, scheduled-task, policy and auth logic over a
//! cases file and writes the results as JSON, in the same shape as the
//! TypeScript runner, so the two can be compared row by row.
//!
//! `cargo run -p t3code-web --example server_settings_logic -- cases.json out.json`

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use serde_json::{Value, json};
use t3code_contracts::{
    ManagedPolicySummary, ModelSelection, ProviderInteractionMode, RuntimeMode, ScheduledTask, ScheduledTaskSchedule,
    ScheduledTaskThreadMode, ScheduledTaskUpsertScheduleWebhookSignatureEncoding as Encoding, ServerSettings,
};
use t3code_web::components::managed_policy::{
    ManagedCapability, is_managed_driver_allowed, is_managed_setting_path, managed_capability_denial,
    managed_instance_locks, managed_runtime_mode_denial,
};
use t3code_web::components::settings::provider_status::{
    get_provider_status_key, get_provider_summary, get_provider_version_advisory_presentation,
    get_provider_version_label, provider_auth_label, AdvisoryEmphasis,
};
use t3code_web::components::settings::scheduled_tasks_settings_logic::*;
use t3code_web::model_selection::{ProviderDriverKind, ProviderIcon};
use t3code_web::provider_auth_delivery::{provider_auth_return_url, read_codex_auth_delivery};
use t3code_web::provider_instances::*;
use t3code_web::provider_models::*;
use t3code_web::provider_skill_search::search_provider_skills;

fn de<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("decode {v}: {e}"))
}

fn s(v: &Value) -> Option<&str> {
    v.as_str()
}

fn selection_json(sel: Option<ModelSelection>) -> Value {
    sel.map_or(Value::Null, |s| json!({"instanceId": s.instance_id, "model": s.model}))
}

fn filter_op(op: &str) -> FilterOp {
    FilterOp::ALL.into_iter().find(|o| o.as_str() == op).expect("op")
}

fn draft_from(v: &Value) -> DraftState {
    let filters = |key: &str| -> Vec<WebhookFilterDraft> {
        v[key]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, f)| WebhookFilterDraft {
                id: i as u64,
                path: f["path"].as_str().unwrap().into(),
                op: filter_op(f["op"].as_str().unwrap()),
                value: f["value"].as_str().unwrap().into(),
            })
            .collect()
    };
    let text = |key: &str| v[key].as_str().unwrap_or_default().to_owned();
    DraftState {
        editing_id: v["editingId"].as_str().map(str::to_owned),
        title: text("title"),
        prompt: text("prompt"),
        enabled: v["enabled"].as_bool().unwrap(),
        schedule_mode: match v["scheduleMode"].as_str().unwrap() {
            "once" => ScheduleMode::Once,
            "interval" => ScheduleMode::Interval,
            "webhook" => ScheduleMode::Webhook,
            _ => ScheduleMode::Fixed,
        },
        interval_minutes: text("intervalMinutes"),
        time_of_day: text("timeOfDay"),
        run_at: text("runAt"),
        saved_run_at: text("savedRunAt"),
        weekdays: v["weekdays"].as_array().unwrap().iter().map(|d| d.as_u64().unwrap()).collect(),
        project_id: text("projectId"),
        thread_id: text("threadId"),
        thread_mode: ScheduledTaskThreadMode::NewThread,
        workspace_mode: WorkspaceMode::Worktree,
        base_ref: text("baseRef"),
        start_from_origin: true,
        existing_worktree_path: text("existingWorktreePath"),
        model_key: text("modelKey"),
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        base_model_selection: None,
        signature_enabled: v["signatureEnabled"].as_bool().unwrap(),
        signature_header: text("signatureHeader"),
        signature_encoding: if v["signatureEncoding"] == "base64" { Encoding::Base64 } else { Encoding::Hex },
        signature_prefix: text("signaturePrefix"),
        signature_secret: text("signatureSecret"),
        max_delivery_age_minutes: text("maxDeliveryAgeMinutes"),
        replay_key: text("replayKey"),
        coalesce_key: text("coalesceKey"),
        coalesce_window_seconds: text("coalesceWindowSeconds"),
        filters: filters("filters"),
        saved_filters: filters("savedFilters"),
    }
}

fn draft_json(d: &DraftState) -> Value {
    let filters = |f: &[WebhookFilterDraft]| -> Value {
        f.iter().map(|f| json!({"path": f.path, "op": f.op.as_str(), "value": f.value})).collect()
    };
    json!({
        "editingId": d.editing_id, "title": d.title, "prompt": d.prompt, "enabled": d.enabled,
        "scheduleMode": match d.schedule_mode { ScheduleMode::Fixed => "fixed", ScheduleMode::Once => "once", ScheduleMode::Interval => "interval", ScheduleMode::Webhook => "webhook" },
        "intervalMinutes": d.interval_minutes, "timeOfDay": d.time_of_day, "runAt": d.run_at, "savedRunAt": d.saved_run_at,
        "weekdays": d.weekdays.iter().collect::<Vec<_>>(), "projectId": d.project_id, "threadId": d.thread_id,
        "threadMode": d.thread_mode.as_str(),
        "workspaceMode": match d.workspace_mode { WorkspaceMode::Root => "root", WorkspaceMode::Worktree => "worktree", WorkspaceMode::ExistingWorktree => "existing_worktree" },
        "baseRef": d.base_ref, "startFromOrigin": d.start_from_origin, "existingWorktreePath": d.existing_worktree_path,
        "modelKey": d.model_key, "runtimeMode": d.runtime_mode.as_str(), "interactionMode": d.interaction_mode.as_str(),
        "baseModelSelection": d.base_model_selection.as_ref().map(|s| serde_json::to_value(s).unwrap()),
        "signatureEnabled": d.signature_enabled, "signatureHeader": d.signature_header, "signatureEncoding": d.signature_encoding.as_str(),
        "signaturePrefix": d.signature_prefix, "signatureSecret": d.signature_secret,
        "replayKey": d.replay_key, "coalesceKey": d.coalesce_key, "coalesceWindowSeconds": d.coalesce_window_seconds,
        "maxDeliveryAgeMinutes": d.max_delivery_age_minutes, "filters": filters(&d.filters), "savedFilters": filters(&d.saved_filters),
    })
}

fn driver_kind(d: &str) -> ProviderDriverKind {
    match d {
        "codex" => ProviderDriverKind::Codex,
        "claudeAgent" => ProviderDriverKind::ClaudeAgent,
        "cursor" => ProviderDriverKind::Cursor,
        "opencode" => ProviderDriverKind::OpenCode,
        "grok" => ProviderDriverKind::Grok,
        "antigravity" => ProviderDriverKind::Antigravity,
        other => ProviderDriverKind::Other(other.to_owned().into()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(&args[1]).expect("cases")).expect("json");
    let mut out = BTreeMap::<&str, Vec<Value>>::new();
    let arr = |k: &str| cases[k].as_array().unwrap().clone();

    out.insert("display", arr("display").iter().map(|r| json!(resolve_provider_instance_display_name(s(&r[0]).unwrap(), s(&r[1]).unwrap(), s(&r[2])))).collect());
    out.insert("initials", arr("initials").iter().map(|r| json!(provider_instance_initials(s(r).unwrap()))).collect());
    out.insert("accent", arr("accent").iter().map(|r| {
        let v = normalize_provider_accent_color(s(r));
        json!({"normalized": v, "fg": v.as_deref().map(provider_accent_foreground_color)})
    }).collect());
    out.insert("ids", arr("ids").iter().map(|r| {
        let base = derive_instance_id(s(&r[0]).unwrap(), s(&r[1]).unwrap());
        let existing: HashSet<String> = de(&r[2]);
        json!({"base": base, "available": derive_available_instance_id(base.clone(), &existing)})
    }).collect());
    out.insert("enabled", arr("enabled").iter().map(|r| {
        let settings: ServerSettings = de(&r[0]);
        json!(resolve_project_provider_instance_enabled(&settings, s(&r[1]), s(&r[2]).unwrap()))
    }).collect());
    out.insert("entries", arr("entries").iter().map(|r| {
        let providers: Vec<Arc<ServerProvider>> = de::<Vec<ServerProvider>>(&r["providers"]).into_iter().map(Arc::new).collect();
        let settings: ServerSettings = de(&r["settings"]);
        let project = s(&r["projectId"]);
        let entries = provider_instance_entries_for(&providers, &settings, project);
        let raw = derive_provider_instance_entries(&providers);
        let selection: Option<ModelSelection> = de(&r["selection"]);
        json!({
            "entries": entries.iter().map(|e| json!({"id": e.instance_id, "enabled": e.enabled, "name": e.display_name, "isDefault": e.is_default,
                "available": e.is_available, "accent": e.accent_color, "agent": e.acp_registry_agent_id, "icon": e.acp_registry_icon_url,
                "ready": is_provider_instance_picker_ready(e), "badge": should_show_instance_badge(e, &entries)})).collect::<Vec<_>>(),
            "hasBadge": has_instance_badge(&raw),
            "projectDefault": selection_json(resolve_project_default_provider_model_selection(&providers, &settings, project)),
            "defaultSel": selection_json(resolve_default_provider_model_selection(&providers, selection.as_ref())),
            "defaultModels": providers.iter().map(|p| json!(get_default_provider_instance_model(&providers, &p.instance_id))).collect::<Vec<_>>(),
        })
    }).collect());
    out.insert("models", arr("models").iter().map(|r| {
        let driver = s(&r["driver"]).unwrap();
        let provider: ServerProvider = de(&r["provider"]);
        let query = s(&r["query"]);
        let models = &provider.models;
        let groups = provider.sub_providers.as_ref().map(|subs| {
            let (ungrouped, groups) = group_models_by_sub_provider(models, subs, |m| m.sub_provider.as_deref());
            json!({"ungrouped": ungrouped.iter().map(|m| &m.slug).collect::<Vec<_>>(),
                   "groups": groups.iter().map(|g| json!([g.sub_provider.name, g.models.iter().map(|m| &m.slug).collect::<Vec<_>>()])).collect::<Vec<_>>()})
        });
        let summary = get_provider_summary(Some(&provider));
        let advisory = get_provider_version_advisory_presentation(&provider, provider.enabled).map(|a| json!({
            "title": a.title.as_ref(), "detail": a.detail.as_ref(), "updateCommand": a.update_command,
            "emphasis": if a.emphasis == AdvisoryEmphasis::Strong { "strong" } else { "normal" }, "targetVersion": a.target_version }));
        let providers = [Arc::new(provider.clone())];
        json!({
            "label": format_provider_driver_kind_label(driver), "runtime": has_runtime_model_catalog(driver),
            "alias": is_runtime_model_default_alias(driver, query), "keeps": keeps_unavailable_model_options(driver, query),
            "selectable": resolve_selectable_model(driver, query, models),
            "caps": serde_json::to_value(get_provider_model_capabilities(models, query, driver, r["includePlan"].as_bool().unwrap())).unwrap(),
            "defaultServer": get_default_server_model(&providers, driver),
            "selectableProvider": resolve_selectable_provider(&providers, Some(query.unwrap_or(driver))),
            "notice": format_provider_update_required_notice(&provider, s(&r["search"]).unwrap(), "en-US").map(|n| n.to_string()),
            "groups": groups,
            "statusKey": get_provider_status_key(Some(&provider)).as_str(),
            "summary": json!({"headline": summary.headline.as_ref(), "detail": summary.detail.as_ref().map(|d| d.to_string())}),
            "version": get_provider_version_label(provider.version.as_deref()),
            "advisory": advisory,
            "authLabel": provider_auth_label(provider.auth.label.as_deref(), provider.auth.r#type.as_deref()).map(|l| l.to_string()),
        })
    }).collect());
    out.insert("semver", arr("semver").iter().map(|r| json!(compare_semver_versions(s(&r[0]).unwrap(), s(&r[1]).unwrap()) as i8)).collect());
    out.insert("schedules", arr("schedules").iter().map(|r| {
        let schedule: ScheduledTaskSchedule = de(&r[0]);
        json!({"label": schedule_label(&schedule, s(&r[1]), s(&r[2])).as_ref(), "zone": schedule_time_zone_label(s(&r[1]), s(&r[2]))})
    }).collect());
    out.insert("drafts", arr("drafts").iter().map(|r| {
        let d = draft_from(r);
        let filters = match parse_webhook_filter_drafts(&d.filters) {
            Ok(f) => serde_json::to_value(f).unwrap(),
            Err(e) => json!({"error": e.as_ref()}),
        };
        json!({
            "errors": schedule_draft_errors(&d).iter().map(|e| json!({"title": e.title.as_ref(), "description": e.description.as_ref()})).collect::<Vec<_>>(),
            "schedule": schedule_from_draft(&d).map(|s| serde_json::to_value(s).unwrap()),
            "filters": filters,
            "age": match parse_max_delivery_age(&d.max_delivery_age_minutes) { Ok(Some(m)) => json!(m), Ok(None) => json!("none"), Err(()) => json!("none") },
            "changed": webhook_filter_drafts_changed(&d.saved_filters, &d.filters),
        })
    }).collect());
    out.insert("tasks", arr("tasks").iter().map(|r| {
        let task: ScheduledTask = de(r);
        let d = task_to_draft(&task);
        json!({"draft": draft_json(&d), "unbound": draft_json(&unbind_draft_from_thread(&d)),
               "moved": draft_json(&move_draft_to_thread(&d, "t9", "p9")),
               "label": schedule_label(&task.schedule, Some("America/Los_Angeles"), None).as_ref()})
    }).collect());
    out.insert("templates", arr("templates").iter().map(|r| json!(is_webhook_dedupe_template(s(r).unwrap()))).collect());
    out.insert("deliveries", arr("deliveries").iter().map(|r| json!(webhook_delivery_label(s(&r[0]).unwrap(), r[1].as_bool().unwrap()).as_ref())).collect());
    out.insert("baserefs", arr("baserefs").iter().map(|r| {
        let refs: Vec<(String, bool)> = de(&r[1]);
        json!(scheduled_task_worktree_base_ref(s(&r[0]).unwrap(), &refs))
    }).collect());
    out.insert("policy_paths", arr("policy_paths").iter().map(|r| {
        let policy: ManagedPolicySummary = serde_json::from_value(json!({"organization": "x", "lockedSettings": r[0]})).unwrap();
        json!(is_managed_setting_path(Some(&policy), s(&r[1]).unwrap()))
    }).collect());
    out.insert("policies", arr("policies").iter().map(|r| {
        let p: ManagedPolicySummary = de(r);
        let p = Some(&p);
        let locks: Vec<Value> = ["codex", "cursor", "pi", "grok"]
            .iter()
            .map(|d| match managed_instance_locks(p, d) {
                Some(l) => json!({"config": l.config_keys, "env": l.environment_names, "whole": l.whole_environment, "reason": l.reason.as_ref()}),
                None => Value::Null,
            })
            .collect();
        let allowed: Vec<bool> = ["codex", "grok"].iter().map(|d| is_managed_driver_allowed(p, d)).collect();
        let runtime: Vec<Option<String>> = RuntimeMode::ALL.iter().map(|m| managed_runtime_mode_denial(p, *m)).collect();
        let caps: Vec<Option<String>> = [
            ManagedCapability::RemoteClients,
            ManagedCapability::Connect,
            ManagedCapability::WebhookTasks,
            ManagedCapability::BrowserCookieImport,
            ManagedCapability::PreviewTunnel,
        ]
        .iter()
        .map(|c| managed_capability_denial(p, *c))
        .collect();
        json!({"locks": locks, "allowed": allowed, "runtime": runtime, "caps": caps})
    }).collect());
    out.insert("auth_urls", arr("auth_urls").iter().map(|r| match read_codex_auth_delivery(s(r).unwrap()) {
        Some(d) => json!({"environmentId": d.environment_id, "instanceId": d.instance_id, "flowId": d.flow_id, "callbackUrl": d.callback_url,
                          "returnHash": d.return_hash, "returnUrl": d.return_url}),
        None => Value::Null,
    }).collect());
    out.insert("return_urls", arr("return_urls").iter().map(|r| json!(provider_auth_return_url(s(r).unwrap()))).collect());
    out.insert("skills", arr("skills").iter().map(|r| {
        let skills: Vec<t3code_contracts::ServerConfigProvidersItemSkillsItem> = de(&r["skills"]);
        json!(search_provider_skills(&skills, s(&r["query"]).unwrap(), r["limit"].as_u64().unwrap() as usize).iter().map(|s| &s.path).collect::<Vec<_>>())
    }).collect());
    out.insert("picker_order", arr("picker_order").iter().map(|r| {
        let items: Vec<String> = de(r);
        let kinds: Vec<(String, ProviderDriverKind)> = items.iter().map(|d| (d.clone(), driver_kind(d))).collect();
        json!(t3code_web::model_ordering::order_claude_before_openai(&kinds, |(_, k)| k).into_iter().map(|(d, _)| d).collect::<Vec<_>>())
    }).collect());
    out.insert("icons", arr("icons").iter().map(|r| {
        let icon = ProviderIcon::for_model(&driver_kind(s(&r[0]).unwrap()), s(&r[1]));
        let driver = s(&r[0]).unwrap();
        // `for_model` folds the three overrides into one icon; split it back per check.
        json!({"meta": driver == "opencode" && icon == ProviderIcon::Meta,
               "grok": driver == "cursor" && icon == ProviderIcon::Grok,
               "gemini": driver == "cursor" && icon == ProviderIcon::Gemini})
    }).collect());
    std::fs::write(&args[2], serde_json::to_string_pretty(&out).unwrap()).expect("write");
}
