//! How turn items read in the timeline's work log, from the contract types alone
//! (`packages/client-runtime/src/work-log`): tool output text, whether a row has detail to
//! open or fetch, failure detection from output, compaction labels, live thought lines and
//! command display text. The UI maps these onto its row model; grouping and group summaries
//! live with the rows (`apps/web/src/components/chat/work_log_presentation.rs`).

pub mod command_label;
pub mod item_detail;
pub mod presentation;
