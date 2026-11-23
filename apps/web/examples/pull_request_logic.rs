//! Runs the ported pull request and issue logic over the cases the TypeScript
//! runner generated and writes the results in the shape it writes, so the two
//! can be compared row by row.
//! Usage: pull_request_logic <cases.json> <out.json>

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde_json::{Value, json};
use t3code_client_runtime::change_request_links as links;
use t3code_client_runtime::issue_list as il;
use t3code_client_runtime::issue_presentation::issue_write_access;
use t3code_client_runtime::issue_selection as isel;
use t3code_client_runtime::pull_request_checks as ch;
use t3code_client_runtime::pull_request_diff_fold::{DiffFoldOverride, PullRequestDiffFolds};
use t3code_client_runtime::pull_request_editing as ed;
use t3code_client_runtime::pull_request_query as q;
use t3code_client_runtime::pull_request_reactions as rx;
use t3code_client_runtime::pull_request_reviewers as rv;
use t3code_client_runtime::pull_request_selection::{SelectionDraft, change_batches};
use t3code_client_runtime::review_guide_tour as tour;
use t3code_client_runtime::saved_views::saved_view_name_error;
use t3code_contracts::*;
use t3code_web::components::pull_request::pull_request_detail_logic as d;
use t3code_web::components::pull_request::pull_request_list_logic as l;

fn de<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("decode {e}: {v}"))
}

fn s(v: &Value) -> Option<&str> {
    v.as_str()
}

fn row(v: &Value) -> l::Row {
    let env = EnvironmentId::new(v["environmentId"].as_str().unwrap_or(""));
    Arc::new(l::PullRequestRow::new(env, de(v)))
}

fn rows(v: &Value) -> Vec<l::Row> {
    v.as_array().map(|a| a.iter().map(row).collect()).unwrap_or_default()
}

fn keys(rows: &[l::Row]) -> Vec<String> {
    rows.iter().map(|r| l::pull_request_entry_key(r)).collect()
}

fn groups_json(groups: &[l::PullRequestGroup]) -> Value {
    json!(groups.iter().map(|g| json!([g.key.as_str(), keys(&g.entries)])).collect::<Vec<_>>())
}

fn filters_json(f: &PullRequestListFilters) -> Value {
    serde_json::to_value(f).unwrap()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cases: BTreeMap<String, Vec<Value>> = serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
    let mut out: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    let mut run = |name: &'static str, f: &dyn Fn(&Value) -> Value| {
        out.insert(name, cases[name].iter().map(f).collect());
    };

    run("query", &|c| {
        let p = q::parse_pull_request_query(c.as_str().unwrap());
        json!({ "text": p.text, "filters": filters_json(&p.filters) })
    });
    run("draftChange", &|c| {
        let (query, filters) = q::apply_pull_request_draft_filter_change(s(&c["query"]).unwrap(), &de(&c["menu"]), &de(&c["next"]));
        json!({ "query": query, "filters": filters_json(&filters) })
    });
    run("reactions", &|c| {
        let reactions: Vec<IssueCommentReactionsItem> = de(&c["reactions"]);
        let pending: rx::PendingReactions = c["pending"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (de::<IssueCommentReactionsItemContent>(&p[0]), p[1].as_bool().unwrap()))
            .collect();
        json!({ "applied": rx::apply_pending_pull_request_reactions(&reactions, &pending), "signature": rx::pull_request_reactions_signature(&reactions) })
    });
    run("canReact", &|c| {
        let kind: PullRequestCommentKind = de(&c["kind"]);
        json!(rx::can_react(c["reactions"].as_bool(), &de(&c["provider"]), kind))
    });
    run("reviewers", &|c| {
        let loaded: Option<PullRequestReviewerCandidateList> = de(&c["loaded"]);
        let data: Option<PullRequestReviewerCandidateList> = de(&c["search"]["data"]);
        let input = rv::ReviewerSearchInput { loaded: loaded.as_ref(), searchable: c["searchable"].as_bool().unwrap(), query: s(&c["query"]).unwrap(), debounced_query: s(&c["debounced"]).unwrap() };
        let search = rv::ReviewerSearchState { data: data.as_ref(), is_pending: c["search"]["isPending"].as_bool().unwrap(), error: s(&c["search"]["error"]) };
        let v = rv::reviewer_candidate_view(&input, &search);
        json!({
            "candidates": v.candidates.iter().map(|x| format!("{}:{}", x.kind.as_str(), x.id)).collect::<Vec<_>>(),
            "searchPending": v.search_pending,
            "searchError": v.search_error,
            "truncation": v.truncation.map(|t| t.as_str()),
            "hostQuery": rv::host_reviewer_query(&input),
        })
    });
    run("selection", &|c| {
        let host: Vec<String> = de(&c["host"]);
        let mut draft = SelectionDraft::<String>::new();
        for op in c["ops"].as_array().unwrap() {
            let k = op[0].as_str().unwrap();
            draft = draft.toggle(k, k.to_owned(), op[1].as_bool().unwrap());
        }
        let changes = draft.changes(|k, _| host.iter().any(|h| h == k));
        let n = c["ops"].as_array().unwrap().len() * 7;
        json!({
            "add": changes.add,
            "remove": changes.remove,
            "selected": (["a", "b", "c", "d"].map(|k| draft.is_selected(k, host.iter().any(|h| h == k)))),
            "batches": change_batches(&(0..n).collect::<Vec<_>>()).iter().map(Vec::len).collect::<Vec<_>>(),
        })
    });
    run("checks", &|c| {
        let checks: Vec<PullRequestCheck> = de(c);
        let g = ch::group_pull_request_checks(&checks);
        let names = |v: &[&PullRequestCheck]| v.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        json!({
            "classify": ch::classify_pull_request_checks(&checks).as_str(),
            "describe": d::describe_pull_request_checks(&checks).as_ref(),
            "summarize": d::summarize_pull_request_checks(&checks).as_ref(),
            "interval": ch::pull_request_checks_refresh_interval_ms(&checks),
            "group": { "attention": names(&g.attention), "running": names(&g.running), "completed": names(&g.completed) },
        })
    });
    run("detailChecks", &|c| {
        let detail: Option<PullRequestDetail> = de(&c["detail"]);
        let checks: Option<PullRequestChecks> = de(&c["checks"]);
        match ch::resolve_pull_request_detail_checks(detail.as_ref(), checks.as_ref(), c["newer"].as_bool().unwrap()) {
            None => Value::Null,
            Some(r) => json!({ "state": r.state, "checks": r.checks.len() }),
        }
    });
    run("folds", &|c| {
        let mut folds = PullRequestDiffFolds::default();
        for op in c["ops"].as_array().unwrap() {
            if let Some(all) = op["all"].as_str() {
                let o = if all == "folded" { DiffFoldOverride::Folded } else { DiffFoldOverride::Expanded };
                let hidden: Option<Vec<String>> = de(&op["hidden"]);
                folds = folds.fold_all(o, hidden.as_deref());
            } else {
                folds = folds.with_fold(op["test"].as_bool().unwrap(), op["key"].as_str().unwrap(), op["collapsed"].as_bool());
            }
        }
        let viewed: Vec<String> = de(&c["viewed"]);
        json!((["a", "b", "t1", "t2"].map(|k| folds.is_collapsed(k, k.starts_with('t'), viewed.iter().any(|v| v == k)))))
    });
    run("editing", &|c| {
        let detail: PullRequestDetail = de(&c["detail"]);
        let comment: PullRequestComment = de(&c["comment"]);
        let thread = PullRequestReviewThread {
            id: "t".into(),
            path: "p".into(),
            line: None,
            side: PullRequestDiffSide::Right,
            is_resolved: false,
            is_outdated: false,
            comments: vec![],
            comment_count: None,
            next_comments_cursor: None,
            can_resolve: c["thread"]["canResolve"].as_bool(),
        };
        let allowed = d::allowed_pull_request_merge_methods(&detail);
        let checks_state = if detail.checks.is_empty() { None } else { Some(de::<PullRequestChecksState>(&c["checksState"])) };
        let perm = |a| detail.viewer_permissions.actions.contains(&a);
        let primary = d::resolve_pull_request_primary_control(d::PrimaryControlInput {
            state: detail.state,
            is_draft: detail.is_draft,
            mergeability: detail.mergeability,
            checks_state,
            auto_merge_enabled: detail.auto_merge_enabled,
            has_merge_method: !allowed.is_empty(),
            can_merge: perm(PullRequestAction::Merge),
            can_mark_ready: perm(PullRequestAction::Ready),
            can_enable_auto_merge: perm(PullRequestAction::EnableAutoMerge),
        });
        let freshness = d::resolve_base_freshness(&detail).map(|(behind, methods)| json!({ "behindBy": behind, "methods": methods }));
        json!({
            "changeRequest": ed::can_edit_pull_request_change_request(&detail),
            "maintainer": ed::can_edit_pull_request_maintainer_edits(&detail),
            "comment": ed::can_edit_pull_request_comment(&detail, comment.kind, comment.author.as_ref().map(|a| a.login.as_str()), c["canEdit"].as_bool()),
            "resolve": ed::can_resolve_pull_request_thread(&detail, &thread),
            "react": rx::can_react_pull_request_comment(&detail, comment.kind),
            "primary": primary.map(|p| p.as_str()),
            "allowed": allowed,
            "method": d::resolve_pull_request_merge_method(&allowed, de(&c["current"]), de(&c["projectDefault"]), de(&c["last"])),
            "conflicting": d::is_pull_request_conflicting(&detail),
            "deleteBranch": d::can_delete_pull_request_branch(&detail),
            "freshness": freshness,
            "threadAction": d::resolve_thread_panel_pull_request_action(&detail).map(|a| a.as_str()),
        })
    });
    run("drafts", &|c| {
        let mut draft: Option<ed::DescriptionDraft> = None;
        let mut trace = Vec::new();
        for op in c["ops"].as_array().unwrap() {
            let key = op["key"].as_str().unwrap();
            draft = match op["op"].as_str().unwrap() {
                "start" => Some(ed::start_description_save(draft.as_ref(), key, op["observedAt"].as_f64(), op["body"].as_str().unwrap())),
                "settle" => ed::settle_description_save(draft.as_ref(), key, op["saved"].as_bool().unwrap()),
                _ => ed::description_draft_in_force(draft.as_ref(), key, op["observedAt"].as_f64()),
            };
            trace.push(draft.as_ref().map(|d| json!({ "key": d.key, "observedAt": d.observed_at, "body": d.body, "saves": d.saves })));
        }
        json!(trace)
    });
    run("staleReply", &|c| {
        let o = |v: &Value| ed::ObservedPullRequest {
            state: de(&v["state"]),
            updated_at: v["updatedAt"].as_str().map(|s| Box::leak(s.to_owned().into_boxed_str()) as &str),
            observed_at: v["observedAt"].as_f64(),
        };
        json!(ed::is_stale_pull_request_reply(&o(&c["shown"]), &o(&c["reply"])))
    });
    run("savedViews", &|c| {
        let views: Vec<(String, String)> =
            c["views"].as_array().unwrap().iter().map(|v| (v["id"].as_str().unwrap().into(), v["name"].as_str().unwrap().into())).collect();
        json!(saved_view_name_error(c["name"].as_str().unwrap(), views.iter().map(|(a, b)| (a.as_str(), b.as_str())), c["editing"].as_str()).map(|e| e.message()))
    });
    run("tour", &|c| {
        let file: tour::FileDiff = de(&c["file"]);
        let stop: PullRequestReviewGuideStop = de(&c["stop"]);
        let rows = tour::file_diff_rows(&file);
        let excerpt = tour::excerpt_rows(&rows, &stop.excerpts[0]);
        let letters = tour::letter_stop_notes(&stop);
        let step = c["step"].as_array().unwrap();
        let count = tour::stop_file_paths(&stop).len();
        json!({
            "step": tour::step_tour_position(step[0].as_u64().unwrap() as usize, step[1].as_i64().unwrap() > 0, step[2].as_u64().unwrap() as usize),
            "files": tour::stop_file_paths(&stop),
            "subline": d::stop_subline(count).as_ref(),
            "rows": rows.iter().map(|r| json!([r.kind.as_str(), r.old_line, r.new_line, r.old_at, r.new_at, r.text, r.hunk])).collect::<Vec<_>>(),
            "excerpt": excerpt.len(),
            "patch": tour::rows_to_patch("a.ts", "a.ts", excerpt),
            "expansion": tour::excerpt_expansion_old_contents(c["newContents"].as_str().unwrap(), excerpt),
            "letters": {
                "lettered": letters.lettered.iter().map(|(m, _)| m.clone()).collect::<Vec<_>>(),
                "byExcerpt": letters.by_excerpt.iter().map(|e| e.iter().map(|(line, ms)| json!([line, ms.iter().map(|(m, t)| json!([m, t])).collect::<Vec<_>>()])).collect::<Vec<_>>()).collect::<Vec<_>>(),
            },
        })
    });
    run("issueWrite", &|c| {
        let detail: IssueDetail = de(&c["detail"]);
        let a = issue_write_access(&detail, c["canWrite"].as_bool().unwrap());
        json!({
            "blocked": a.blocked.map(|b| b.message()),
            "comment": a.comment, "edit": a.edit, "labels": a.labels, "assignees": a.assignees, "react": a.react,
            "states": a.states.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
        })
    });
    run("issuePicks", &|c| {
        let detail: IssueDetail = de(&c["detail"]);
        let names: Vec<String> = detail.labels.iter().map(|l| l.name.clone()).collect();
        let mut ld = SelectionDraft::<IssueLabelCandidate>::new();
        for op in c["labelOps"].as_array().unwrap() {
            let name = op[0].as_str().unwrap();
            ld = ld.toggle(name, IssueLabelCandidate { name: name.into(), color: None, description: None }, names.iter().any(|n| n == name));
        }
        let mut ad = SelectionDraft::<IssueAssigneeCandidate>::new();
        for cand in c["assigneeOps"].as_array().unwrap() {
            let candidate = IssueAssigneeCandidate { id: cand["id"].as_str().unwrap().into(), login: cand["login"].as_str().unwrap().into(), name: None, avatar_url: None, is_bot: None };
            let held = isel::issue_assignee_held(&detail, &candidate);
            ad = ad.toggle(&candidate.id.clone(), candidate, held);
        }
        let labels = isel::issue_labels_after_picks(&detail, &ld);
        let written: Option<Vec<String>> = de(&c["written"]);
        json!({
            "labels": labels,
            "assignees": isel::issue_assignees_after_picks(&detail, &ad),
            "write": isel::issue_write_after_picks(labels.as_deref(), &names, written.as_deref()),
        })
    });
    run("issueList", &|c| {
        let answers: Vec<(EnvironmentId, IssueListResult)> =
            c["answers"].as_array().unwrap().iter().map(|a| (EnvironmentId::new(a[0].as_str().unwrap()), de(&a[1]))).collect();
        let more: IssueListResult = de(&c["more"]);
        let held_map = il::hold_issue_pages(&answers, &HashMap::new());
        let mut held: Vec<(EnvironmentId, il::HeldIssuePage)> =
            answers.iter().map(|(e, _)| (e.clone(), held_map[e].clone())).collect();
        let page0 = held[0].1.clone();
        let asked = il::issue_cursors_for_host(&page0.next_cursors, "github.com");
        let continued = il::continue_issue_page(&page0, if c["fail"].as_bool().unwrap() { Err("nope") } else { Ok(&more) }, &asked);
        held[0].1 = continued.clone();
        let sort: IssueListSort = de(&c["sort"]);
        let groups = il::group_issues(&held, c["query"].as_str().unwrap(), sort, |_, _| None);
        json!({
            "merged": il::merge_issue_list_page(&answers[0].1.entries, &more.entries).iter().map(il::issue_entry_key).collect::<Vec<_>>(),
            "continued": {
                "entries": continued.entries.iter().map(|e| format!("{}|{}", e.url, e.key)).collect::<Vec<_>>(),
                "errors": continued.errors.len(),
                "next": continued.next_cursors,
                "failure": continued.more_failure,
            },
            "groups": groups.iter().map(|g| json!({
                "key": il::issue_tracker_key(g.tracker.tracker.as_str(), &g.tracker.host),
                "configured": g.tracker.configured,
                "rows": g.rows.iter().map(|r| format!("{}|{}", r.environment_id.as_str(), r.entry.url)).collect::<Vec<_>>(),
                "errors": g.errors.len(),
                "more": g.more.iter().map(|e| e.as_str().to_owned()).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    });
    run("links", &|c| {
        let url = c[0].as_str().unwrap();
        let other = c[1].as_str().unwrap();
        let cr = links::parse_change_request_url(url).map(|(k, l)| {
            let mut v = json!({ "kind": k.as_str(), "host": l.host, "repository": l.repository, "number": l.number, "repositoryUrl": l.repository_url });
            if let Some(a) = l.authority {
                v["authority"] = json!(a);
            }
            v
        });
        let issue = links::parse_issue_url(url, None).map(|(k, l)| {
            let mut v = json!({ "kind": k.as_str(), "host": l.host, "scope": l.scope, "key": l.key });
            if let Some(a) = l.scope_alias {
                v["scopeAlias"] = json!(a);
            }
            v
        });
        json!({ "cr": cr, "issue": issue, "same": links::is_same_change_request(url, other) })
    });
    run("crUrl", &|c| {
        let kind = links::SourceControlHostKind::from_wire(c["kind"].as_str().unwrap()).unwrap();
        json!(links::change_request_url(kind, links::ChangeRequestUrlInput {
            host: c["host"].as_str().unwrap(),
            repository: c["repository"].as_str().unwrap(),
            number: c["number"].as_u64().unwrap(),
            remote_url: c["remoteUrl"].as_str(),
            web_url: c["webUrl"].as_str(),
        }))
    });
    run("listFilters", &|c| {
        let entry: PullRequestListEntry = de(&c["entry"]);
        json!(l::matches_pull_request_filters(&entry, &de(&c["filters"]), c["viewer"].as_str()))
    });
    run("listGroups", &|c| {
        let known = rows(&c["known"]);
        let baseline = (!c["baseline"].is_null()).then(|| rows(&c["baseline"]));
        let providers: Vec<PullRequestProviderSummary> = de(&c["providers"]);
        let viewers: l::PullRequestViewers = de(&c["viewers"]);
        let filters: PullRequestListFilters = de(&c["filters"]);
        let involvement: PullRequestInvolvement = de(&c["involvement"]);
        let authored = (!c["authored"].is_null()).then(|| rows(&c["authored"]));
        let reviewing = (!c["reviewing"].is_null()).then(|| rows(&c["reviewing"]));
        let text = c["text"].as_str().unwrap();
        let (entries, groups) = l::pull_request_groups(l::PullRequestGroupsInput {
            known: &known,
            baseline: baseline.as_deref(),
            providers: &providers,
            viewers: &viewers,
            involvement,
            local_filters: &filters,
            has_local_filters: q::has_pull_request_filters(&filters),
            text,
            answered_locally: c["answeredLocally"].as_bool().unwrap(),
            baseline_empty: c["baselineEmpty"].as_bool().unwrap(),
            authored: authored.as_deref(),
            reviewing: reviewing.as_deref(),
            partitions_wanted: c["partitionsWanted"].as_bool().unwrap(),
        });
        let sort = l::PullRequestListSort::from_str(c["sort"].as_str().unwrap()).unwrap();
        let sorted = l::sort_pull_request_groups(groups.clone(), sort, text, involvement);
        let (authors, labels) = l::collect_pull_request_list_facets(&known, de(&c["facetState"]));
        json!({
            "entries": keys(&entries),
            "groups": groups_json(&groups),
            "sorted": groups_json(&sorted),
            "ranked": keys(&l::rank_pull_request_matches(&known, text)),
            "facets": {
                "authors": authors.iter().map(|a| json!([a.actor.login, a.count, a.merged_count])).collect::<Vec<_>>(),
                "labels": labels.iter().map(|f| json!([f.label.name, f.label.color, f.count])).collect::<Vec<_>>(),
            },
        })
    });
    run("listMerge", &|c| {
        let answers: Vec<(EnvironmentId, Arc<PullRequestListResult>)> =
            c.as_array().unwrap().iter().map(|a| (EnvironmentId::new(a[0].as_str().unwrap()), Arc::new(de(&a[1])))).collect();
        let m = l::merge_pull_request_lists(&answers).unwrap();
        json!({
            "viewers": m.viewers,
            "providers": m.providers,
            "entries": keys(&m.entries),
            "truncated": m.truncated,
            "truncatedEnvironments": m.truncated_environments.iter().map(|e| e.as_str().to_owned()).collect::<Vec<_>>(),
            "nextCursors": m.next_cursors,
        })
    });
    run("listActions", &|c| {
        let entry = row(&c["entry"]);
        let host_actions: Vec<PullRequestAction> = de(&c["hostActions"]);
        let action: PullRequestAction = de(&c["action"]);
        let reply: Option<PullRequestSummary> = de(&c["reply"]);
        let now_ms = 1_767_225_600_000.0;
        let o = l::pull_request_override_after_action(&entry.entry, action, now_ms, "2026-01-01T00:00:00.000Z", 7, reply.as_ref());
        let overrides: HashMap<String, l::PullRequestListOverride> = o.iter().map(|o| (l::pull_request_entry_key(&entry), o.clone())).collect();
        let applied = l::apply_pull_request_overrides(std::slice::from_ref(&entry), &overrides, de(&c["listState"]));
        let settled = l::settle_pull_request_overrides(&overrides, std::slice::from_ref(&entry), now_ms + c["later"].as_f64().unwrap());
        json!({
            "quick": l::pull_request_quick_actions(&entry.entry, &host_actions).iter().map(|q| q.action().as_str()).collect::<Vec<_>>(),
            "override": o.map(|o| json!({ "state": o.state, "isDraft": o.is_draft, "updatedAt": o.updated_at, "fromReply": o.from_reply })),
            "applied": applied.iter().map(|r| json!([r.entry.state, r.entry.is_draft, r.entry.updated_at])).collect::<Vec<_>>(),
            "settled": settled.len(),
        })
    });
    run("conversation", &|c| {
        let comments: Vec<PullRequestComment> = de(&c["comments"]);
        let threads: Vec<PullRequestReviewThread> = de(&c["threads"]);
        let threads: Vec<d::ConversationThread> = threads.iter().map(Into::into).collect();
        let commits: Vec<PullRequestCommit> = de(&c["commits"]);
        let items = d::pull_request_conversation_items(&comments, &threads);
        let filtered = d::filter_pull_request_conversation_items(&items, c["bots"].as_bool().unwrap(), c["resolved"].as_bool().unwrap());
        let timeline = d::build_pull_request_timeline(d::TimelineSource {
            created_at: c["createdAt"].as_str().unwrap(),
            author: de(&c["author"]),
            commits: &commits,
            comments: &comments,
            threads: &threads,
            merged_at: c["mergedAt"].as_str(),
            closed_at: c["closedAt"].as_str(),
        });
        let desc = |i: &d::ConversationItem| match i {
            d::ConversationItem::Comment(c) => json!(["comment", c.id]),
            d::ConversationItem::Thread { root, replies, thread, more_replies, has_more_replies, url } => {
                json!(["thread", thread.id, root.id, replies.iter().map(|r| r.id.clone()).collect::<Vec<_>>(), more_replies, has_more_replies, url])
            }
        };
        let kind = |k: d::TimelineEventKind| match k {
            d::TimelineEventKind::Opened => "opened",
            d::TimelineEventKind::Commit => "commit",
            d::TimelineEventKind::Comment => "comment",
            d::TimelineEventKind::Review => "review",
            d::TimelineEventKind::Merged => "merged",
            d::TimelineEventKind::Closed => "closed",
        };
        let rows = d::group_pull_request_timeline_conversations(timeline.clone());
        json!({
            "items": items.iter().map(desc).collect::<Vec<_>>(),
            "filtered": filtered.iter().map(desc).collect::<Vec<_>>(),
            "timeline": timeline.iter().map(|e| json!([e.id, kind(e.kind), e.body, e.thread.as_ref().map(|t| t.replies.iter().map(|r| r.id.clone()).collect::<Vec<_>>())])).collect::<Vec<_>>(),
            "rows": rows.iter().map(|r| match r {
                d::TimelineRow::Event(e) => json!(["event", e.id]),
                d::TimelineRow::Comments(es) => json!(["comments", es.iter().map(|e| e.id.clone()).collect::<Vec<_>>()]),
            }).collect::<Vec<_>>(),
            "outcomes": d::latest_pull_request_review_outcomes(&comments, &commits).iter().map(|o| json!([o.key, o.outcome.as_str(), o.at, o.stale])).collect::<Vec<_>>(),
            "newest": d::newest_pull_request_commit_at(&commits),
            "visible": comments.iter().map(|c| d::visible_body(&c.body)).collect::<Vec<_>>(),
        })
    });

    run("markdown", &|c| {
        use t3code_client_runtime::pull_request_markdown as md;
        fn seg(s: &md::PullRequestBodySegment) -> Value {
            match s {
                md::PullRequestBodySegment::Markdown { id, text, source_offset } => json!(["markdown", id, text, source_offset]),
                md::PullRequestBodySegment::Attachment { id, url, media } => json!(["attachment", id, url, media.as_str()]),
                md::PullRequestBodySegment::Table { id, rows } => json!(["table", id, rows.iter().map(|r| r.iter().map(|c| json!([c.header, c.segments.iter().map(seg).collect::<Vec<_>>()])).collect::<Vec<_>>()).collect::<Vec<_>>()]),
            }
        }
        let body = c["body"].as_str().unwrap();
        json!({
            "segments": md::split_pull_request_body(body).iter().map(seg).collect::<Vec<_>>(),
            "emoji": md::expand_github_emoji_aliases(body),
            "media": md::resolve_change_request_media_url(c["media"].as_str().unwrap(), "https://git.cafe/o/r/pull/5"),
        })
    });

    std::fs::write(&args[2], serde_json::to_string(&out).unwrap()).unwrap();

    // Every wire fixture of a type the data sources return decodes through the same types.
    if let Some(path) = args.get(3) {
        let wire: BTreeMap<String, Vec<Value>> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        fn check<T: serde::de::DeserializeOwned>(values: Option<&Vec<Value>>) -> (usize, usize) {
            let values = values.map(Vec::as_slice).unwrap_or(&[]);
            (values.iter().filter(|v| serde_json::from_value::<T>((*v).clone()).is_ok()).count(), values.len())
        }
        let rows = [
            ("PullRequestListStatsResult", check::<PullRequestListStatsResult>(wire.get("PullRequestListStatsResult"))),
            ("PullRequestFilterOptions", check::<PullRequestFilterOptions>(wire.get("PullRequestFilterOptions"))),
            ("PullRequestSummary", check::<PullRequestSummary>(wire.get("PullRequestSummary"))),
            ("PullRequestLinkedThreadsResult", check::<PullRequestLinkedThreadsResult>(wire.get("PullRequestLinkedThreadsResult"))),
            ("PullRequestPreview", check::<PullRequestPreview>(wire.get("PullRequestPreview"))),
            ("PullRequestReviewGuideResult", check::<PullRequestReviewGuideResult>(wire.get("PullRequestReviewGuideResult"))),
            ("PullRequestThreadCommentsResult", check::<PullRequestThreadCommentsResult>(wire.get("PullRequestThreadCommentsResult"))),
            ("PullRequestDiffFileContentsResult", check::<PullRequestDiffFileContentsResult>(wire.get("PullRequestDiffFileContentsResult"))),
            ("PullRequestFilesViewedResult", check::<PullRequestFilesViewedResult>(wire.get("PullRequestFilesViewedResult"))),
            ("PullRequestChangeBreakdown", check::<PullRequestChangeBreakdown>(wire.get("PullRequestChangeBreakdown"))),
            ("PullRequestReviewerCandidateList", check::<PullRequestReviewerCandidateList>(wire.get("PullRequestReviewerCandidateList"))),
            ("PullRequestLabelCandidateList", check::<PullRequestLabelCandidateList>(wire.get("PullRequestLabelCandidateList"))),
            ("IssueCommentsPage", check::<IssueCommentsPage>(wire.get("IssueCommentsPage"))),
            ("IssueTemplateList", check::<IssueTemplateList>(wire.get("IssueTemplateList"))),
            ("IssueCandidates", check::<IssueCandidates>(wire.get("IssueCandidates"))),
            ("IssueLinkedThreadsResult", check::<IssueLinkedThreadsResult>(wire.get("IssueLinkedThreadsResult"))),
            ("IssueCreateResult", check::<IssueCreateResult>(wire.get("IssueCreateResult"))),
            ("IssueTrackerConnectionList", check::<IssueTrackerConnectionList>(wire.get("IssueTrackerConnectionList"))),
        ];
        println!("| wire fixture type | decoded | values |\n|---|---:|---:|");
        for (name, (ok, total)) in rows {
            println!("| {name} | {ok} | {total} |");
        }
    }
}
