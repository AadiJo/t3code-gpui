//! The scheduled task editor's pure logic. Ports
//! `scheduledTasksSettings.logic.ts`, `scheduledTaskTimeZone.ts`,
//! `scheduledTaskWebhook.ts`, `scheduledTaskWorktree.ts` and the webhook
//! schema checks in `contracts/scheduledTask.ts`.
//!
//! Local times use the process time zone (`TZ`), as the browser does.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::SharedString;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use jiff::Timestamp;
use t3code_contracts::{
    ModelSelection, OrchestrationV2ThreadLaunchWorkspaceStrategy as WorkspaceStrategy, ProviderInteractionMode,
    RuntimeMode, ScheduledTask, ScheduledTaskSchedule, ScheduledTaskThreadMode, ScheduledTaskUpsertSchedule,
    ScheduledTaskUpsertScheduleFixedTime, ScheduledTaskUpsertScheduleInterval, ScheduledTaskUpsertScheduleOnce,
    ScheduledTaskUpsertScheduleWebhook, ScheduledTaskUpsertScheduleWebhookDedupe,
    ScheduledTaskUpsertScheduleWebhookSignature, ScheduledTaskUpsertScheduleWebhookSignatureEncoding as SignatureEncoding,
    ScheduledTaskWebhookFilter, ScheduledTaskWebhookFilterEqualsOrNotEqualsOrStartsWith as ValueFilter,
    ScheduledTaskWebhookFilterEqualsOrNotEqualsOrStartsWithOp as ValueOp, ScheduledTaskWebhookFilterExists,
    ScheduledTaskWebhookFilterIn,
};

use super::scheduled_tasks_settings_zone_names::{GENERIC, NAMED};
use crate::i18n::t;

/// `MAX_WEBHOOK_DELIVERY_AGE_MINUTES`: one day.
pub const MAX_WEBHOOK_DELIVERY_AGE_MINUTES: u64 = 24 * 60;
/// `MAX_WEBHOOK_FILTERS`.
pub const MAX_WEBHOOK_FILTERS: usize = 16;
/// `DEFAULT_WEBHOOK_PROMPT`.
pub const DEFAULT_WEBHOOK_PROMPT: &str = "Handle this webhook:\n{{body}}";
/// `WEBHOOK_SIGNATURE_DEFAULTS`.
pub const SIGNATURE_HEADER_DEFAULT: &str = "x-hub-signature-256";
pub const SIGNATURE_PREFIX_DEFAULT: &str = "sha256=";

fn offset_label(seconds: i32) -> String {
    if seconds == 0 {
        return "GMT".into();
    }
    let sign = if seconds < 0 { '-' } else { '+' };
    let minutes = seconds.unsigned_abs() / 60;
    let (h, m) = (minutes / 60, minutes % 60);
    if m == 0 { format!("GMT{sign}{h}") } else { format!("GMT{sign}{h}:{m:02}") }
}

/// `scheduleTimeZoneLabel`: the zone a fixed-time schedule runs in. With a
/// run time it names that instant's offset (PST/PDT); without one the generic name (PT).
pub fn schedule_time_zone_label(time_zone: Option<&str>, run_at: Option<&str>) -> Option<String> {
    let zone = time_zone?;
    if matches!(zone, "Etc/UTC" | "UTC" | "GMT") {
        return Some("UTC".into());
    }
    let tz = TimeZone::get(zone).ok()?;
    let at = match run_at {
        Some(run_at) => run_at.parse::<Timestamp>().ok()?,
        None => Timestamp::now(),
    };
    if run_at.is_none()
        && let Ok(ix) = GENERIC.binary_search_by(|(z, _)| z.cmp(&zone))
    {
        return Some(GENERIC[ix].1.to_owned());
    }
    let info = tz.to_offset_info(at);
    if let Ok(ix) = NAMED.binary_search_by(|(z, _, _)| z.cmp(&zone)) {
        let (_, standard, daylight) = NAMED[ix];
        return Some(if info.dst().is_dst() { daylight } else { standard }.to_owned());
    }
    Some(offset_label(info.offset().seconds()))
}

/// `weekdayShortLabel` in English, 0 = Sunday.
pub fn weekday_short_label(day: u64) -> &'static str {
    ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].get(day as usize).copied().unwrap_or("")
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// `Intl.DateTimeFormat("en-US", {dateStyle: "medium", timeStyle: "short"})` in the local zone.
pub fn format_once_time(iso: &str) -> String {
    let Ok(ts) = iso.parse::<Timestamp>() else { return "Invalid Date".into() };
    let z = ts.to_zoned_datetime(TimeZone::system());
    let hour = z.hour();
    let (h12, meridiem) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    format!(
        "{} {}, {}, {}:{:02}\u{202f}{}",
        MONTHS[(z.month() - 1) as usize],
        z.day(),
        z.year(),
        h12,
        z.minute(),
        meridiem
    )
}

/// `scheduleLabel`: "Daily at 09:00 PT", "Every 15 min", "On webhook"…
pub fn schedule_label(schedule: &ScheduledTaskSchedule, time_zone: Option<&str>, run_at: Option<&str>) -> SharedString {
    match schedule {
        ScheduledTaskSchedule::Webhook(_) => t!("On webhook"),
        ScheduledTaskSchedule::Once(once) => t!("Once, {time}", time = format_once_time(&once.run_at)),
        ScheduledTaskSchedule::Interval(interval) => {
            let every = interval.every_ms;
            if every % 60_000 == 0 {
                t!("Every {minutes} min", minutes = every / 60_000)
            } else {
                t!("Every {seconds} sec", seconds = (every as f64 / 1000.).round() as u64)
            }
        }
        ScheduledTaskSchedule::FixedTime(fixed) => {
            let weekdays = fixed.weekdays.clone().unwrap_or_default();
            let time = fixed.time_of_day.clone();
            let zone = schedule_time_zone_label(time_zone, run_at);
            if weekdays.is_empty() {
                return match zone {
                    None => t!("Daily at {time}", time = time),
                    Some(zone) => t!("Daily at {time} {zone}", time = time, zone = zone),
                };
            }
            if weekdays.len() == 5 && weekdays.iter().all(|d| (1..=5).contains(d)) {
                return match zone {
                    None => t!("Weekdays at {time}", time = time),
                    Some(zone) => t!("Weekdays at {time} {zone}", time = time, zone = zone),
                };
            }
            let days = weekdays.iter().map(|d| weekday_short_label(*d)).collect::<Vec<_>>().join(", ");
            match zone {
                None => t!("{days} at {time}", days = days, time = time),
                Some(zone) => t!("{days} at {time} {zone}", days = days, time = time, zone = zone),
            }
        }
        ScheduledTaskSchedule::Unknown(_) => SharedString::default(),
    }
}

/// A webhook filter operator (`ScheduledTaskWebhookFilterOp`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterOp {
    Equals,
    NotEquals,
    In,
    StartsWith,
    Exists,
}

impl FilterOp {
    /// `WEBHOOK_FILTER_OPS`, in menu order.
    pub const ALL: [FilterOp; 5] = [Self::Equals, Self::NotEquals, Self::In, Self::StartsWith, Self::Exists];

    pub fn label(self) -> SharedString {
        match self {
            Self::Equals => t!("equals"),
            Self::NotEquals => t!("does not equal"),
            Self::In => t!("is one of"),
            Self::StartsWith => t!("starts with"),
            Self::Exists => t!("exists"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Equals => "equals",
            Self::NotEquals => "notEquals",
            Self::In => "in",
            Self::StartsWith => "startsWith",
            Self::Exists => "exists",
        }
    }
}

/// One editable filter row. `in` takes a comma-separated list; `exists` ignores the value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebhookFilterDraft {
    /// Stable row identity for the form; never sent.
    pub id: u64,
    pub path: String,
    pub op: FilterOp,
    pub value: String,
}

static NEXT_FILTER_ID: AtomicU64 = AtomicU64::new(0);

/// `emptyWebhookFilterDraft`.
pub fn empty_webhook_filter_draft() -> WebhookFilterDraft {
    WebhookFilterDraft {
        id: NEXT_FILTER_ID.fetch_add(1, Ordering::Relaxed) + 1,
        path: String::new(),
        op: FilterOp::Equals,
        value: String::new(),
    }
}

/// `webhookFilterDrafts`: rows for stored filters.
pub fn webhook_filter_drafts(filters: Option<&[ScheduledTaskWebhookFilter]>) -> Vec<WebhookFilterDraft> {
    filters
        .unwrap_or_default()
        .iter()
        .filter_map(|filter| {
            let (path, op, value) = match filter {
                ScheduledTaskWebhookFilter::EqualsOrNotEqualsOrStartsWith(f) => (
                    f.path.clone(),
                    match f.op {
                        ValueOp::Equals => FilterOp::Equals,
                        ValueOp::NotEquals => FilterOp::NotEquals,
                        ValueOp::StartsWith => FilterOp::StartsWith,
                    },
                    f.value.clone(),
                ),
                ScheduledTaskWebhookFilter::In(f) => (f.path.clone(), FilterOp::In, f.value.join(", ")),
                ScheduledTaskWebhookFilter::Exists(f) => (f.path.clone(), FilterOp::Exists, String::new()),
                ScheduledTaskWebhookFilter::Unknown(_) => return None,
            };
            Some(WebhookFilterDraft { id: NEXT_FILTER_ID.fetch_add(1, Ordering::Relaxed) + 1, path, op, value })
        })
        .collect()
}

/// `removeWebhookFilterDraft`: by id, so identities never shift.
pub fn remove_webhook_filter_draft(drafts: &[WebhookFilterDraft], id: u64) -> Vec<WebhookFilterDraft> {
    drafts.iter().filter(|d| d.id != id).cloned().collect()
}

/// `WebhookFilterPath`: trimmed, non-empty, under body, headers or query.
fn is_filter_path(path: &str) -> bool {
    !path.is_empty()
        && ["body", "headers", "query"]
            .iter()
            .any(|root| path.strip_prefix(root).is_some_and(|rest| rest.is_empty() || rest.starts_with('.')))
}

/// `WebhookFilterListValue`: non-empty, no commas, no surrounding whitespace.
fn is_list_value(value: &str) -> bool {
    !value.is_empty() && !value.contains(',') && value.trim() == value
}

/// `parseWebhookFilterDrafts`: the stored filters, or the first row that would not save.
pub fn parse_webhook_filter_drafts(drafts: &[WebhookFilterDraft]) -> Result<Vec<ScheduledTaskWebhookFilter>, SharedString> {
    if drafts.len() > MAX_WEBHOOK_FILTERS {
        return Err(t!("Use at most {max} filters.", max = MAX_WEBHOOK_FILTERS));
    }
    let mut filters = Vec::with_capacity(drafts.len());
    for (index, draft) in drafts.iter().enumerate() {
        let number = index + 1;
        let path = draft.path.trim().to_owned();
        let filter = match draft.op {
            FilterOp::Exists => is_filter_path(&path)
                .then(|| ScheduledTaskWebhookFilter::Exists(ScheduledTaskWebhookFilterExists { path: path.clone() })),
            FilterOp::In => {
                let value: Vec<String> =
                    draft.value.split(',').map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned).collect();
                (is_filter_path(&path) && !value.is_empty() && value.iter().all(|v| is_list_value(v)))
                    .then(|| ScheduledTaskWebhookFilter::In(ScheduledTaskWebhookFilterIn { path: path.clone(), value }))
            }
            op => is_filter_path(&path).then(|| {
                ScheduledTaskWebhookFilter::EqualsOrNotEqualsOrStartsWith(ValueFilter {
                    path: path.clone(),
                    op: match op {
                        FilterOp::NotEquals => ValueOp::NotEquals,
                        FilterOp::StartsWith => ValueOp::StartsWith,
                        _ => ValueOp::Equals,
                    },
                    value: draft.value.clone(),
                })
            }),
        };
        match filter {
            Some(filter) => filters.push(filter),
            None => {
                return Err(match draft.op {
                    FilterOp::Exists => {
                        t!("Filter {number} needs a path under body, headers or query.", number = number)
                    }
                    FilterOp::In => t!(
                        "Filter {number} needs a path under body, headers or query, and at least one value.",
                        number = number
                    ),
                    _ => t!("Filter {number} needs a path under body, headers or query, and a value.", number = number),
                });
            }
        }
    }
    Ok(filters)
}

/// `webhookFilterDraftsChanged`: only edited filters are sent on save.
pub fn webhook_filter_drafts_changed(saved: &[WebhookFilterDraft], current: &[WebhookFilterDraft]) -> bool {
    saved.len() != current.len()
        || saved.iter().zip(current).any(|(a, b)| {
            a.path != b.path || a.op != b.op || (a.op != FilterOp::Exists && a.value != b.value)
        })
}

/// `parseMaxDeliveryAge`: `Ok(None)` for blank (no limit), `Err` for an invalid limit.
pub fn parse_max_delivery_age(value: &str) -> Result<Option<u64>, ()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match parse_js_integer(trimmed) {
        Some(minutes) if minutes > 0 && minutes as u64 <= MAX_WEBHOOK_DELIVERY_AGE_MINUTES => Ok(Some(minutes as u64)),
        _ => Err(()),
    }
}

/// `Number(text)` when it is an integer: decimal, with an optional exponent or fraction of zeros.
fn parse_js_integer(text: &str) -> Option<i64> {
    let value: f64 = if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).ok()? as f64
    } else {
        if text.contains(|c: char| c.is_ascii_alphabetic() && c != 'e' && c != 'E') {
            return None;
        }
        text.parse().ok()?
    };
    (value.is_finite() && value.fract() == 0. && value.abs() < 9.0e15).then_some(value as i64)
}

/// `isWebhookDedupeTemplate`: at least one request placeholder and no stray braces.
pub fn is_webhook_dedupe_template(template: &str) -> bool {
    use std::sync::LazyLock;
    static PLACEHOLDER: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\{\{\s*(?:(?:body|headers|query)(?:\.[^{}.\s]+)*|request)\s*\}\}").expect("placeholder regex")
    });
    let has_match = PLACEHOLDER.is_match(template);
    let remainder = PLACEHOLDER.replace_all(template, "");
    has_match && !remainder.contains("{{") && !remainder.contains("}}")
}

/// Which schedule the editor is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleMode {
    Fixed,
    Once,
    Interval,
    Webhook,
}

/// Where a run happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceMode {
    Root,
    Worktree,
    ExistingWorktree,
}

/// The editor's state (`DraftState`).
#[derive(Clone, Debug, PartialEq)]
pub struct DraftState {
    pub editing_id: Option<String>,
    pub title: String,
    pub prompt: String,
    pub enabled: bool,
    pub schedule_mode: ScheduleMode,
    pub interval_minutes: String,
    pub time_of_day: String,
    /// Local `YYYY-MM-DDTHH:MM`.
    pub run_at: String,
    /// The stored ISO run time, sent back unchanged when the field was not edited.
    pub saved_run_at: String,
    pub weekdays: BTreeSet<u64>,
    pub project_id: String,
    pub thread_id: String,
    pub thread_mode: ScheduledTaskThreadMode,
    pub workspace_mode: WorkspaceMode,
    pub base_ref: String,
    pub start_from_origin: bool,
    pub existing_worktree_path: String,
    pub model_key: String,
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
    pub base_model_selection: Option<ModelSelection>,
    pub signature_enabled: bool,
    pub signature_header: String,
    pub signature_encoding: SignatureEncoding,
    pub signature_prefix: String,
    /// Write-only: empty keeps the stored secret.
    pub signature_secret: String,
    pub max_delivery_age_minutes: String,
    pub replay_key: String,
    pub coalesce_key: String,
    pub coalesce_window_seconds: String,
    pub filters: Vec<WebhookFilterDraft>,
    pub saved_filters: Vec<WebhookFilterDraft>,
}

impl DraftState {
    /// A new task's draft (`EMPTY_DRAFT` in `ScheduledTasksSettings.tsx`), weekdays Monday to Friday.
    pub fn new_task(project_id: String) -> Self {
        Self {
            editing_id: None,
            title: String::new(),
            prompt: String::new(),
            enabled: true,
            schedule_mode: ScheduleMode::Fixed,
            interval_minutes: "15".into(),
            time_of_day: "09:00".into(),
            run_at: String::new(),
            saved_run_at: String::new(),
            weekdays: (1..=5).collect(),
            project_id,
            thread_id: String::new(),
            thread_mode: ScheduledTaskThreadMode::NewThread,
            workspace_mode: WorkspaceMode::Worktree,
            base_ref: String::new(),
            start_from_origin: true,
            existing_worktree_path: String::new(),
            model_key: String::new(),
            runtime_mode: RuntimeMode::FullAccess,
            interaction_mode: ProviderInteractionMode::Default,
            base_model_selection: None,
            signature_enabled: false,
            signature_header: SIGNATURE_HEADER_DEFAULT.into(),
            signature_encoding: SignatureEncoding::Hex,
            signature_prefix: SIGNATURE_PREFIX_DEFAULT.into(),
            signature_secret: String::new(),
            max_delivery_age_minutes: String::new(),
            replay_key: String::new(),
            coalesce_key: String::new(),
            coalesce_window_seconds: String::new(),
            filters: Vec::new(),
            saved_filters: Vec::new(),
        }
    }
}

/// A validation error: a toast title and its description.
#[derive(Clone, Debug, PartialEq)]
pub struct DraftError {
    pub title: SharedString,
    pub description: SharedString,
}

/// `toDateTimeLocalValue`: a timestamp as local `YYYY-MM-DDTHH:MM`.
pub fn to_date_time_local_value(ts: Timestamp) -> String {
    ts.to_zoned_datetime(TimeZone::system()).strftime("%Y-%m-%dT%H:%M").to_string()
}

/// `new Date("YYYY-MM-DDTHH:MM")` read as local time.
fn parse_local_date_time(value: &str) -> Option<Timestamp> {
    let civil: DateTime = value.parse().ok()?;
    if value.len() != 16 {
        return None;
    }
    // `compatible` disambiguation matches `Date`: the earlier offset in a repeated hour, forward over a gap.
    Some(civil.to_zoned_datetime(TimeZone::system()).ok()?.timestamp())
}

/// `scheduleDraftErrors`.
pub fn schedule_draft_errors(draft: &DraftState) -> Vec<DraftError> {
    let invalid_run_time = |description: SharedString| vec![DraftError { title: t!("Invalid run time"), description }];
    match draft.schedule_mode {
        ScheduleMode::Once => {
            let Some(run_at) = parse_local_date_time(&draft.run_at) else {
                return invalid_run_time(t!("Pick a date and time."));
            };
            if to_date_time_local_value(run_at) != draft.run_at {
                return invalid_run_time(t!("That time does not exist in your time zone."));
            }
            Vec::new()
        }
        ScheduleMode::Fixed if draft.time_of_day.is_empty() => invalid_run_time(t!("Pick a time.")),
        ScheduleMode::Webhook => {
            if parse_max_delivery_age(&draft.max_delivery_age_minutes).is_err() {
                return vec![DraftError {
                    title: t!("Invalid age limit"),
                    description: t!(
                        "Enter whole minutes from 1 to {max}, or leave it blank.",
                        max = MAX_WEBHOOK_DELIVERY_AGE_MINUTES
                    ),
                }];
            }
            let mut errors = Vec::new();
            for (description, value) in [
                (t!("Duplicate request key must contain a valid request placeholder."), &draft.replay_key),
                (t!("Coalesce key must contain a valid request placeholder."), &draft.coalesce_key),
            ] {
                if !value.trim().is_empty() && !is_webhook_dedupe_template(value.trim()) {
                    errors.push(DraftError { title: t!("Invalid webhook settings"), description });
                }
            }
            let window = draft.coalesce_window_seconds.trim();
            if !window.is_empty() && !parse_js_integer(window).is_some_and(|w| (10..=3600).contains(&w)) {
                errors.push(DraftError {
                    title: t!("Invalid webhook settings"),
                    description: t!("Coalesce window must be whole seconds from 10 to 3600."),
                });
            }
            errors
        }
        _ => Vec::new(),
    }
}

/// `webhookDeliveryLabel`.
pub fn webhook_delivery_label(outcome: &str, pending_window: bool) -> SharedString {
    match outcome {
        "accepted" if pending_window => t!("Opened window"),
        "accepted" => t!("Ran", context = "webhook delivery outcome"),
        "dispatch_failed" => t!("Run failed"),
        "duplicate" => t!("Duplicate", context = "webhook delivery outcome"),
        "coalesced" => t!("Coalesced"),
        "filtered" => t!("Filtered", context = "webhook delivery outcome"),
        "rejected_signature" => t!("Bad signature"),
        "disabled" => t!("Task paused"),
        "rate_limited" => t!("Rate limited"),
        "expired" => t!("Too old"),
        other => other.replace('_', " ").into(),
    }
}

/// `scheduleFromDraft`: the schedule to save, or `None` while the draft has errors.
pub fn schedule_from_draft(draft: &DraftState) -> Option<ScheduledTaskUpsertSchedule> {
    if !schedule_draft_errors(draft).is_empty() {
        return None;
    }
    Some(match draft.schedule_mode {
        ScheduleMode::Once => {
            let saved = draft.saved_run_at.parse::<Timestamp>().ok().filter(|_| !draft.saved_run_at.is_empty());
            if let Some(saved) = saved
                && to_date_time_local_value(saved) == draft.run_at
            {
                return Some(ScheduledTaskUpsertSchedule::Once(ScheduledTaskUpsertScheduleOnce {
                    run_at: draft.saved_run_at.clone(),
                }));
            }
            // A repeated hour keeps the saved time's offset; otherwise read it as local time.
            let with_saved_offset = saved.and_then(|saved| {
                let offset = TimeZone::system().to_offset(saved);
                let civil: DateTime = draft.run_at.parse().ok()?;
                let ts = offset.to_timestamp(civil).ok()?;
                (to_date_time_local_value(ts) == draft.run_at).then_some(ts)
            });
            let ts = with_saved_offset.or_else(|| parse_local_date_time(&draft.run_at))?;
            ScheduledTaskUpsertSchedule::Once(ScheduledTaskUpsertScheduleOnce { run_at: to_iso_string(ts) })
        }
        ScheduleMode::Webhook => {
            let max_delivery_age_minutes = parse_max_delivery_age(&draft.max_delivery_age_minutes).ok()?;
            let replay_key = draft.replay_key.trim();
            let coalesce_key = draft.coalesce_key.trim();
            let window = draft.coalesce_window_seconds.trim();
            let coalesce_window_seconds = (!window.is_empty()).then(|| parse_js_integer(window).map(|w| w as u64)).flatten();
            let filters = parse_webhook_filter_drafts(&draft.filters).ok()?;
            let secret = draft.signature_secret.trim();
            ScheduledTaskUpsertSchedule::Webhook(ScheduledTaskUpsertScheduleWebhook {
                signature: draft.signature_enabled.then(|| ScheduledTaskUpsertScheduleWebhookSignature {
                    header: draft.signature_header.trim().to_owned(),
                    encoding: draft.signature_encoding,
                    prefix: draft.signature_prefix.clone(),
                    secret: (!secret.is_empty()).then(|| secret.to_owned()),
                    secret_ref: None,
                }),
                max_delivery_age_minutes,
                dedupe: Some(ScheduledTaskUpsertScheduleWebhookDedupe {
                    replay_key: (!replay_key.is_empty()).then(|| replay_key.to_owned()),
                    coalesce_key: (!coalesce_key.is_empty()).then(|| coalesce_key.to_owned()),
                    coalesce_window_seconds,
                }),
                filters: webhook_filter_drafts_changed(&draft.saved_filters, &draft.filters).then_some(filters),
            })
        }
        ScheduleMode::Interval => {
            let minutes: f64 = draft.interval_minutes.trim().parse().unwrap_or(f64::NAN);
            ScheduledTaskUpsertSchedule::Interval(ScheduledTaskUpsertScheduleInterval {
                every_ms: (minutes * 60_000.).round() as u64,
            })
        }
        ScheduleMode::Fixed => {
            let every_day = draft.weekdays.is_empty() || draft.weekdays.len() == 7;
            ScheduledTaskUpsertSchedule::FixedTime(ScheduledTaskUpsertScheduleFixedTime {
                time_of_day: draft.time_of_day.clone(),
                weekdays: (!every_day).then(|| draft.weekdays.iter().copied().collect()),
            })
        }
    })
}

/// `Date#toISOString`: UTC with milliseconds.
pub fn to_iso_string(ts: Timestamp) -> String {
    ts.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// `scheduledTaskThreadMode`.
pub fn scheduled_task_thread_mode(task: &ScheduledTask) -> ScheduledTaskThreadMode {
    task.thread_mode.unwrap_or(if task.thread_id.is_none() {
        ScheduledTaskThreadMode::NewThread
    } else {
        ScheduledTaskThreadMode::Continue
    })
}

/// `taskToDraft`: the editor state for a stored task.
pub fn task_to_draft(task: &ScheduledTask) -> DraftState {
    let schedule = &task.schedule;
    let webhook = match schedule {
        ScheduledTaskSchedule::Webhook(w) => Some(w),
        _ => None,
    };
    let filters = webhook_filter_drafts(webhook.and_then(|w| w.filters.as_deref()));
    let weekdays = match schedule {
        ScheduledTaskSchedule::FixedTime(f) if f.weekdays.as_ref().is_some_and(|w| !w.is_empty()) => {
            f.weekdays.iter().flatten().copied().collect()
        }
        _ => (0..7).collect(),
    };
    let (workspace_mode, base_ref, start_from_origin, existing) = match &task.workspace_strategy {
        WorkspaceStrategy::Worktree(w) => {
            (WorkspaceMode::Worktree, w.base_ref.clone(), w.start_from_origin.unwrap_or(false), String::new())
        }
        WorkspaceStrategy::ExistingWorktree(w) => {
            (WorkspaceMode::ExistingWorktree, String::new(), true, w.worktree_path.clone())
        }
        _ => (WorkspaceMode::Root, String::new(), true, String::new()),
    };
    let signature = webhook.and_then(|w| w.signature.as_ref());
    let dedupe = webhook.and_then(|w| w.dedupe.as_ref());
    DraftState {
        editing_id: Some(task.id.to_string()),
        title: task.title.clone(),
        prompt: task.prompt.clone(),
        enabled: task.enabled,
        schedule_mode: match schedule {
            ScheduledTaskSchedule::Once(_) => ScheduleMode::Once,
            ScheduledTaskSchedule::Interval(_) => ScheduleMode::Interval,
            ScheduledTaskSchedule::Webhook(_) => ScheduleMode::Webhook,
            _ => ScheduleMode::Fixed,
        },
        interval_minutes: match schedule {
            ScheduledTaskSchedule::Interval(i) => format_js_number((i.every_ms as f64 / 60_000.).max(1.)),
            _ => "15".into(),
        },
        time_of_day: match schedule {
            ScheduledTaskSchedule::FixedTime(f) => format!("{:0>5}", f.time_of_day),
            _ => "09:00".into(),
        },
        run_at: match schedule {
            ScheduledTaskSchedule::Once(o) => {
                o.run_at.parse::<Timestamp>().map(to_date_time_local_value).unwrap_or_else(|_| "NaN-NaN-NaNTNaN:NaN".into())
            }
            _ => String::new(),
        },
        saved_run_at: match schedule {
            ScheduledTaskSchedule::Once(o) => o.run_at.clone(),
            _ => String::new(),
        },
        weekdays,
        project_id: task.project_id.to_string(),
        thread_id: task.thread_id.as_ref().map(|t| t.to_string()).unwrap_or_default(),
        thread_mode: scheduled_task_thread_mode(task),
        workspace_mode,
        base_ref,
        start_from_origin,
        existing_worktree_path: existing,
        model_key: format!("{}:{}", task.model_selection.instance_id, task.model_selection.model),
        runtime_mode: task.runtime_mode,
        interaction_mode: task.interaction_mode,
        base_model_selection: Some(task.model_selection.clone()),
        signature_enabled: signature.is_some(),
        signature_header: signature.map_or_else(|| SIGNATURE_HEADER_DEFAULT.into(), |s| s.header.clone()),
        signature_encoding: signature.map_or(SignatureEncoding::Hex, |s| {
            SignatureEncoding::from_wire(s.encoding.as_str()).unwrap_or(SignatureEncoding::Hex)
        }),
        signature_prefix: signature.map_or_else(|| SIGNATURE_PREFIX_DEFAULT.into(), |s| s.prefix.clone()),
        signature_secret: String::new(),
        replay_key: dedupe.and_then(|d| d.replay_key.clone()).unwrap_or_default(),
        coalesce_key: dedupe.and_then(|d| d.coalesce_key.clone()).unwrap_or_default(),
        coalesce_window_seconds: dedupe.and_then(|d| d.coalesce_window_seconds).map(|w| w.to_string()).unwrap_or_default(),
        max_delivery_age_minutes: webhook.and_then(|w| w.max_delivery_age_minutes).map(|m| m.to_string()).unwrap_or_default(),
        saved_filters: filters.clone(),
        filters,
    }
}

/// `String(number)` for the values the editor shows.
fn format_js_number(value: f64) -> String {
    if value.fract() == 0. && value.abs() < 1e21 { format!("{}", value as i64) } else { format!("{value}") }
}

/// `unbindDraftFromThread`: new threads stay out of the shared checkout.
pub fn unbind_draft_from_thread(draft: &DraftState) -> DraftState {
    if draft.thread_id.is_empty() {
        return draft.clone();
    }
    let mut next = draft.clone();
    next.thread_id.clear();
    if next.workspace_mode == WorkspaceMode::Root {
        next.workspace_mode = WorkspaceMode::Worktree;
        next.start_from_origin = true;
    }
    next
}

/// `changeDraftProject`.
pub fn change_draft_project(draft: &DraftState, project_id: &str) -> DraftState {
    if project_id == draft.project_id {
        return draft.clone();
    }
    let mut next = unbind_draft_from_thread(draft);
    next.project_id = project_id.to_owned();
    next
}

/// `moveDraftToThread`.
pub fn move_draft_to_thread(draft: &DraftState, thread_id: &str, project_id: &str) -> DraftState {
    let mut next = draft.clone();
    next.thread_id = thread_id.to_owned();
    next.project_id = project_id.to_owned();
    if next.thread_mode == ScheduledTaskThreadMode::NewThread {
        next.thread_mode = ScheduledTaskThreadMode::Continue;
    }
    next
}

/// `scheduledTaskWorktreeBaseRef`: blank means the default branch, else `main`/`master`.
pub fn scheduled_task_worktree_base_ref(base_ref: &str, refs: &[(String, bool)]) -> String {
    let picked = base_ref.trim();
    if !picked.is_empty() {
        return picked.to_owned();
    }
    if let Some((name, _)) = refs.iter().find(|(_, default)| *default) {
        return name.clone();
    }
    for name in ["main", "master"] {
        if let Some((found, _)) = refs.iter().find(|(r, _)| r == name || *r == format!("origin/{name}")) {
            return found.clone();
        }
    }
    "main".into()
}
