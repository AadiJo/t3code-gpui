//! `packages/shared/src/usageMerge.ts`: merges usage summaries from several
//! environments into one set of totals, counting a transcript directory that
//! two environments both read only once.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde::Serialize;
use t3code_contracts::{
    EnvironmentId, SidebarAutoSettleAfterDays, UsageCostSource, UsageProviderKind, UsageSourceFingerprint,
    UsageSourceStatus, UsageSummary, UsageSummaryBucketsItem, UsageSummarySourcesItem, UsageTokenTotals,
};

use crate::usage_format::parse_instant_ms;

/// `USAGE_CONTRACT_VERSION` in `packages/contracts/src/usage.ts`.
pub const USAGE_CONTRACT_VERSION: f64 = 6.0;
/// `USAGE_MERGE_COMPATIBLE_SINCE` in `packages/contracts/src/usage.ts`.
pub const USAGE_MERGE_COMPATIBLE_SINCE: f64 = 4.0;

/// A wire number, which may arrive as `"NaN"` or `"Infinity"`.
pub fn number(value: &SidebarAutoSettleAfterDays) -> f64 {
    use t3code_contracts::SidebarAutoSettleAfterDaysText as Text;
    match value {
        SidebarAutoSettleAfterDays::Number(value) => *value,
        SidebarAutoSettleAfterDays::Text(Text::Infinity) => f64::INFINITY,
        SidebarAutoSettleAfterDays::Text(Text::InfinityAlt) => f64::NEG_INFINITY,
        SidebarAutoSettleAfterDays::Text(Text::NaN) => f64::NAN,
    }
}

/// One environment's answer to `server.getUsageSummary`.
#[derive(Clone, Debug)]
pub struct EnvironmentUsage {
    pub environment_id: EnvironmentId,
    pub label: String,
    pub summary: Arc<UsageSummary>,
}

/// Cost and tokens of one provider or one period cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostTokens {
    pub cost_usd: f64,
    pub total_tokens: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTotals {
    pub provider: UsageProviderKind,
    pub cost_usd: f64,
    pub total_tokens: f64,
    pub records: f64,
    pub sessions: f64,
    pub cost_share: f64,
    pub token_share: f64,
}

/// Token counts by kind; `reasoning` is part of `output`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenTotals {
    pub uncached_input_tokens: f64,
    pub cached_input_tokens: f64,
    pub cache_creation_tokens: f64,
    pub output_tokens: f64,
    pub reasoning_tokens: f64,
}

impl TokenTotals {
    fn add(&mut self, totals: &UsageTokenTotals) {
        self.uncached_input_tokens += totals.uncached_input_tokens as f64;
        self.cached_input_tokens += totals.cached_input_tokens as f64;
        self.cache_creation_tokens += totals.cache_creation_tokens as f64;
        self.output_tokens += totals.output_tokens as f64;
        self.reasoning_tokens += totals.reasoning_tokens as f64;
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTotals {
    pub model: String,
    pub provider: UsageProviderKind,
    pub cost_usd: f64,
    pub total_tokens: f64,
    pub tokens: TokenTotals,
    pub records: f64,
    pub unpriced_records: f64,
    pub unpriced_tokens: f64,
    pub cost_share: f64,
    pub token_share: f64,
}

impl ModelTotals {
    /// When every record lacks rates the cost is unknown, not zero.
    pub fn is_cost_unknown(&self) -> bool {
        self.records > 0.0 && self.unpriced_records >= self.records
    }
}

/// One day (or, with `hour_start`, one hour) of the chart.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeriodTotals {
    pub day: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hour_start: Option<String>,
    pub cost_usd: f64,
    pub total_tokens: f64,
    pub by_provider: BTreeMap<UsageProviderKind, CostTokens>,
}

impl PeriodTotals {
    /// The period key the chart columns are looked up by.
    pub fn key(&self) -> &str {
        self.hour_start.as_deref().unwrap_or(&self.day)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostQuality {
    pub provider_reported_share: f64,
    pub model_priced_share: f64,
    pub unpriced_share: f64,
    pub cache_savings_usd: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryCost {
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
    pub unsplit: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedCost {
    pub standard: f64,
    pub fast: f64,
    pub ultrafast: f64,
    pub premium: f64,
}

/// Whether the server's contract is older or newer than this client's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MismatchDirection {
    ServerBehind,
    ClientBehind,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageContractMismatch {
    pub environment_id: EnvironmentId,
    pub direction: MismatchDirection,
    pub contract_version: f64,
}

/// `MergedUsage`: what the Usage page shows for the selected environments.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergedUsage {
    pub cost_usd: f64,
    pub uncached_input_tokens: f64,
    pub cached_input_tokens: f64,
    pub cache_creation_tokens: f64,
    pub output_tokens: f64,
    pub reasoning_tokens: f64,
    pub total_tokens: f64,
    pub records: f64,
    pub sessions: f64,
    pub providers: Vec<ProviderTotals>,
    pub models: Vec<ModelTotals>,
    pub daily: Vec<PeriodTotals>,
    pub hourly: Vec<PeriodTotals>,
    pub cost_quality: CostQuality,
    pub category_cost: CategoryCost,
    pub speed_cost: SpeedCost,
    pub duplicate_sources: Vec<String>,
    pub contributing_environments: Vec<EnvironmentId>,
    pub contract_mismatches: Vec<UsageContractMismatch>,
}

/// Whether this client can merge a summary of `version`.
pub fn is_compatible_usage_contract_version(version: f64, expected: f64) -> bool {
    version >= USAGE_MERGE_COMPATIBLE_SINCE && version <= expected
}

// volumeId distinguishes machines sharing a hostname and home path.
fn fingerprint_key(fp: &UsageSourceFingerprint) -> String {
    format!("{} {} {} {}", fp.host_id, fp.provider.as_str(), fp.resolved_home_path, fp.volume_id)
}

/// Union-find over source keys: two sources are one directory when their
/// legacy key or their `physicalSourceId` matches.
struct SourceIdentity {
    parent: HashMap<String, String>,
}

impl SourceIdentity {
    fn new(environments: &[&EnvironmentUsage]) -> Self {
        let mut this = Self { parent: HashMap::new() };
        for environment in environments {
            for source in &environment.summary.sources {
                let fp = &source.fingerprint;
                let Some(physical) = &fp.physical_source_id else { continue };
                let legacy = this.find(&fingerprint_key(fp));
                let physical = this.find(&format!("\0{}\0{physical}", fp.provider.as_str()));
                if legacy != physical {
                    this.parent.insert(legacy, physical);
                }
            }
        }
        this
    }

    fn find(&mut self, key: &str) -> String {
        match self.parent.get(key).cloned() {
            None => key.to_owned(),
            Some(next) if next == key => next,
            Some(next) => {
                let root = self.find(&next);
                self.parent.insert(key.to_owned(), root.clone());
                root
            }
        }
    }

    fn key(&mut self, fp: &UsageSourceFingerprint) -> String {
        self.find(&fingerprint_key(fp))
    }
}

/// Indexes a bucket by its environment and position, standing in for the
/// object identity the TypeScript keeps buckets in sets by.
type BucketRef = (usize, usize);

fn buckets_for_source<'a>(
    summary: &'a UsageSummary,
    source: &UsageSummarySourcesItem,
) -> impl Iterator<Item = (usize, &'a UsageSummaryBucketsItem)> {
    let provider = source.fingerprint.provider;
    let single = summary.sources.iter().filter(|entry| entry.fingerprint.provider == provider).count() == 1;
    let home = source.fingerprint.resolved_home_path.clone();
    summary.buckets.iter().enumerate().filter(move |(_, bucket)| {
        bucket.provider == provider
            && (bucket.source_path.as_deref() == Some(home.as_str()) || (bucket.source_path.is_none() && single))
    })
}

fn bucket_key(bucket: &UsageSummaryBucketsItem) -> (String, Option<String>, UsageProviderKind, String) {
    (bucket.day.clone(), bucket.hour_start.clone(), bucket.provider, bucket.model.clone())
}

fn read_at(environment: &EnvironmentUsage) -> f64 {
    parse_instant_ms(&environment.summary.read_at).map_or(f64::NAN, |ms| ms as f64)
}

struct Claims {
    /// Source key to (environment index, source index) of the reading that counts.
    owner: HashMap<String, (usize, usize)>,
    supplemental: HashMap<usize, HashSet<BucketRef>>,
    sessions: HashMap<String, f64>,
    duplicates: Vec<String>,
}

fn claim_sources(environments: &[&EnvironmentUsage], identity: &mut SourceIdentity) -> Claims {
    let mut claims =
        Claims { owner: HashMap::new(), supplemental: HashMap::new(), sessions: HashMap::new(), duplicates: Vec::new() };
    let mut order: Vec<usize> = (0..environments.len()).collect();
    order.sort_by(|&a, &b| {
        let (ra, rb) = (read_at(environments[a]), read_at(environments[b]));
        let (ra, rb) = (if ra.is_nan() { 0.0 } else { ra }, if rb.is_nan() { 0.0 } else { rb });
        rb.total_cmp(&ra).then_with(|| environments[a].environment_id.as_str().cmp(environments[b].environment_id.as_str()))
    });

    for status in [UsageSourceStatus::OkType, UsageSourceStatus::Partial, UsageSourceStatus::Failed] {
        for &index in &order {
            let environment = environments[index];
            for (source_index, source) in environment.summary.sources.iter().enumerate() {
                if source.status != status {
                    continue;
                }
                let key = identity.key(&source.fingerprint);
                if claims.owner.contains_key(&key) {
                    claims.duplicates.push(format!("{}: {}", environment.label, source.fingerprint.resolved_home_path));
                    continue;
                }
                claims.owner.insert(key.clone(), (index, source_index));
                claims.sessions.insert(key, source.distinct_sessions as f64);
            }
        }
    }

    // Keep cells unique to newer partial scans; aggregates cannot reconcile overlapping records.
    let mut seen_by_key: HashMap<String, HashSet<(String, Option<String>, UsageProviderKind, String)>> = HashMap::new();
    for &index in &order {
        let environment = environments[index];
        for source in &environment.summary.sources {
            if source.status != UsageSourceStatus::Partial {
                continue;
            }
            let key = identity.key(&source.fingerprint);
            let Some(&(owner_env, owner_source)) = claims.owner.get(&key) else { continue };
            let owner = environments[owner_env];
            let owner_source = &owner.summary.sources[owner_source];
            // A scan under another model filter counts different records.
            if owner_source.status != UsageSourceStatus::OkType
                || read_at(environment) <= read_at(owner)
                || environment.summary.model_filter != owner.summary.model_filter
            {
                continue;
            }
            let seen = seen_by_key.entry(key.clone()).or_insert_with(|| {
                buckets_for_source(&owner.summary, owner_source).map(|(_, bucket)| bucket_key(bucket)).collect()
            });
            let mut added = false;
            for (bucket_index, bucket) in buckets_for_source(&environment.summary, source) {
                if seen.insert(bucket_key(bucket)) {
                    claims.supplemental.entry(index).or_default().insert((index, bucket_index));
                    added = true;
                }
            }
            if added {
                let sessions = claims.sessions.entry(key).or_default();
                *sessions = sessions.max(source.distinct_sessions as f64);
            }
        }
    }
    claims
}

/// The buckets this environment contributes, and its distinct sessions per provider.
fn owned_contribution(
    index: usize,
    environment: &EnvironmentUsage,
    claims: &Claims,
    identity: &mut SourceIdentity,
) -> (Vec<usize>, Vec<(UsageProviderKind, f64)>) {
    let mut owned_providers = HashSet::new();
    let mut owned_sources = HashSet::new();
    let mut sessions: Vec<(UsageProviderKind, f64)> = Vec::new();
    for (source_index, source) in environment.summary.sources.iter().enumerate() {
        if source.status == UsageSourceStatus::Missing {
            continue;
        }
        let key = identity.key(&source.fingerprint);
        // By source, not environment: one server can reach a directory through two mounts.
        if claims.owner.get(&key) == Some(&(index, source_index)) {
            let provider = source.fingerprint.provider;
            owned_providers.insert(provider);
            owned_sources.insert((provider, source.fingerprint.resolved_home_path.clone()));
            let count = claims.sessions.get(&key).copied().unwrap_or(source.distinct_sessions as f64);
            match sessions.iter_mut().find(|(p, _)| *p == provider) {
                Some((_, total)) => *total += count,
                None => sessions.push((provider, count)),
            }
        }
    }
    let supplemental = claims.supplemental.get(&index);
    let buckets = environment
        .summary
        .buckets
        .iter()
        .enumerate()
        .filter(|(bucket_index, bucket)| {
            supplemental.is_some_and(|set| set.contains(&(index, *bucket_index)))
                || match &bucket.source_path {
                    None => owned_providers.contains(&bucket.provider),
                    Some(path) => owned_sources.contains(&(bucket.provider, path.clone())),
                }
        })
        .map(|(bucket_index, _)| bucket_index)
        .collect();
    (buckets, sessions)
}

/// Sum of the four token kinds; reasoning is already part of output.
fn bucket_tokens(bucket: &UsageSummaryBucketsItem) -> f64 {
    let t = &bucket.totals;
    (t.uncached_input_tokens + t.cached_input_tokens + t.cache_creation_tokens + t.output_tokens) as f64
}

/// An insertion-ordered map, matching JavaScript `Map` iteration.
struct Ordered<K, V> {
    index: HashMap<K, usize>,
    entries: Vec<(K, V)>,
}

impl<K: std::hash::Hash + Eq + Clone, V> Ordered<K, V> {
    fn new() -> Self {
        Self { index: HashMap::new(), entries: Vec::new() }
    }

    fn entry(&mut self, key: &K, default: impl FnOnce() -> V) -> &mut V {
        let position = match self.index.get(key) {
            Some(&position) => position,
            None => {
                self.entries.push((key.clone(), default()));
                self.index.insert(key.clone(), self.entries.len() - 1);
                self.entries.len() - 1
            }
        };
        &mut self.entries[position].1
    }
}

#[derive(Default)]
struct ProviderAccumulator {
    cost_usd: f64,
    total_tokens: f64,
    records: f64,
    sessions: f64,
}

struct ModelAccumulator {
    provider: UsageProviderKind,
    cost_usd: f64,
    total_tokens: f64,
    tokens: TokenTotals,
    records: f64,
    unpriced_records: f64,
    unpriced_tokens: f64,
}

fn add_period(period: &mut PeriodTotals, provider: UsageProviderKind, cost: f64, tokens: f64) {
    period.cost_usd += cost;
    period.total_tokens += tokens;
    let cell = period.by_provider.entry(provider).or_default();
    cell.cost_usd += cost;
    cell.total_tokens += tokens;
}

/// `mergeUsage`: one set of totals for `environments`. Summaries on an
/// incompatible contract version are left out and reported as mismatches.
pub fn merge_usage(environments: &[EnvironmentUsage], expected_contract_version: f64) -> MergedUsage {
    if environments.is_empty() {
        return MergedUsage::default();
    }
    let mut current: Vec<&EnvironmentUsage> = Vec::new();
    let mut contract_mismatches = Vec::new();
    for environment in environments {
        let version = number(&environment.summary.contract_version);
        if is_compatible_usage_contract_version(version, expected_contract_version) {
            current.push(environment);
        } else {
            contract_mismatches.push(UsageContractMismatch {
                environment_id: environment.environment_id.clone(),
                direction: if version < expected_contract_version {
                    MismatchDirection::ServerBehind
                } else {
                    MismatchDirection::ClientBehind
                },
                contract_version: version,
            });
        }
    }

    let mut identity = SourceIdentity::new(&current);
    let claims = claim_sources(&current, &mut identity);

    let mut merged = MergedUsage { contract_mismatches, duplicate_sources: claims.duplicates.clone(), ..Default::default() };
    let (mut provider_reported_records, mut unpriced_records) = (0.0, 0.0);
    let mut providers: Ordered<UsageProviderKind, ProviderAccumulator> = Ordered::new();
    let mut models: Ordered<(UsageProviderKind, String), ModelAccumulator> = Ordered::new();
    let mut daily: BTreeMap<String, PeriodTotals> = BTreeMap::new();
    let mut hourly: BTreeMap<String, PeriodTotals> = BTreeMap::new();

    for (index, environment) in current.iter().enumerate() {
        let (buckets, sessions_by_provider) = owned_contribution(index, environment, &claims, &mut identity);
        if !buckets.is_empty() {
            merged.contributing_environments.push(environment.environment_id.clone());
        }
        for (provider, sessions) in sessions_by_provider {
            merged.sessions += sessions;
            if sessions == 0.0 {
                continue;
            }
            providers.entry(&provider, Default::default).sessions += sessions;
        }
        for bucket_index in buckets {
            let bucket = &environment.summary.buckets[bucket_index];
            let tokens = bucket_tokens(bucket);
            let cost = number(&bucket.cost_usd);
            let records = bucket.records as f64;
            merged.cost_usd += cost;
            merged.cost_quality.cache_savings_usd += number(&bucket.cache_savings_usd);
            merged.uncached_input_tokens += bucket.totals.uncached_input_tokens as f64;
            merged.cached_input_tokens += bucket.totals.cached_input_tokens as f64;
            merged.cache_creation_tokens += bucket.totals.cache_creation_tokens as f64;
            merged.output_tokens += bucket.totals.output_tokens as f64;
            merged.reasoning_tokens += bucket.totals.reasoning_tokens as f64;
            merged.records += records;
            unpriced_records += bucket.unpriced_records as f64;
            if bucket.cost_source == UsageCostSource::ProviderReported {
                provider_reported_records += records;
            }
            if let Some(category) = &bucket.category_cost_usd {
                merged.category_cost.input += number(&category.input);
                merged.category_cost.cache_read += number(&category.cache_read);
                merged.category_cost.cache_write += number(&category.cache_write);
                merged.category_cost.output += number(&category.output);
            }
            merged.speed_cost.fast += bucket.fast_cost_usd.as_ref().map_or(0.0, number);
            merged.speed_cost.ultrafast += bucket.ultrafast_cost_usd.as_ref().map_or(0.0, number);
            merged.speed_cost.premium += bucket.speed_premium_usd.as_ref().map_or(0.0, number);

            let provider = providers.entry(&bucket.provider, Default::default);
            provider.cost_usd += cost;
            provider.total_tokens += tokens;
            provider.records += records;

            let model = models.entry(&(bucket.provider, bucket.model.clone()), || ModelAccumulator {
                provider: bucket.provider,
                cost_usd: 0.0,
                total_tokens: 0.0,
                tokens: TokenTotals::default(),
                records: 0.0,
                unpriced_records: 0.0,
                unpriced_tokens: 0.0,
            });
            model.cost_usd += cost;
            model.total_tokens += tokens;
            model.tokens.add(&bucket.totals);
            model.records += records;
            model.unpriced_records += bucket.unpriced_records as f64;
            if let Some(unpriced) = bucket.unpriced_tokens {
                model.unpriced_tokens += unpriced as f64;
            } else if bucket.records > 0 {
                model.unpriced_tokens += tokens * bucket.unpriced_records as f64 / records;
            }

            let day = daily.entry(bucket.day.clone()).or_insert_with(|| PeriodTotals {
                day: bucket.day.clone(),
                hour_start: None,
                cost_usd: 0.0,
                total_tokens: 0.0,
                by_provider: BTreeMap::new(),
            });
            add_period(day, bucket.provider, cost, tokens);
            if let Some(hour_start) = &bucket.hour_start {
                let hour = hourly.entry(hour_start.clone()).or_insert_with(|| PeriodTotals {
                    day: bucket.day.clone(),
                    hour_start: Some(hour_start.clone()),
                    cost_usd: 0.0,
                    total_tokens: 0.0,
                    by_provider: BTreeMap::new(),
                });
                add_period(hour, bucket.provider, cost, tokens);
            }
        }
    }

    let total = merged.uncached_input_tokens + merged.cached_input_tokens + merged.cache_creation_tokens + merged.output_tokens;
    merged.total_tokens = total;
    let cost = merged.cost_usd;
    let share = |part: f64, whole: f64| if whole == 0.0 { 0.0 } else { part / whole };

    merged.providers = providers
        .entries
        .into_iter()
        .map(|(provider, totals)| ProviderTotals {
            provider,
            cost_usd: totals.cost_usd,
            total_tokens: totals.total_tokens,
            records: totals.records,
            sessions: totals.sessions,
            cost_share: share(totals.cost_usd, cost),
            token_share: share(totals.total_tokens, total),
        })
        .collect();
    merged.providers.sort_by(|a, b| b.cost_usd.total_cmp(&a.cost_usd));

    merged.models = models
        .entries
        .into_iter()
        .map(|((_, model), totals)| ModelTotals {
            model,
            provider: totals.provider,
            cost_usd: totals.cost_usd,
            total_tokens: totals.total_tokens,
            tokens: totals.tokens,
            records: totals.records,
            unpriced_records: totals.unpriced_records,
            unpriced_tokens: totals.unpriced_tokens,
            cost_share: share(totals.cost_usd, cost),
            token_share: share(totals.total_tokens, total),
        })
        .collect();
    merged.models.sort_by(|a, b| b.cost_usd.total_cmp(&a.cost_usd).then(b.total_tokens.total_cmp(&a.total_tokens)));

    merged.daily = daily.into_values().collect();
    merged.hourly = hourly.into_values().collect();

    let records = merged.records;
    merged.cost_quality.provider_reported_share = share(provider_reported_records, records);
    merged.cost_quality.unpriced_share = share(unpriced_records, records);
    merged.cost_quality.model_priced_share =
        if records == 0.0 { 0.0 } else { (records - provider_reported_records - unpriced_records) / records };
    let category = &mut merged.category_cost;
    category.unsplit = (cost - category.input - category.cache_read - category.cache_write - category.output).max(0.0);
    let speed = &mut merged.speed_cost;
    speed.standard = (cost - speed.fast - speed.ultrafast).max(0.0);
    merged
}
