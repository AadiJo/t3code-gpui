//! How turn items read in the timeline's work log, from the contract types alone
//! (`packages/client-runtime/src/work-log`, `packages/shared/src/toolActivity.ts`,
//! `t3McpToolPresentation.ts`): tool output text, whether a row has detail to open or fetch,
//! failure detection, compaction labels, live thought lines, command display text and program
//! names, tool classification, and T3 Code's own MCP tools. Text the user reads is built in
//! the web crate (`apps/web/src/work_entry_labels.rs`) so it goes through translation.

pub mod command_label;
pub mod item_detail;
pub mod presentation;
pub mod t3_tools;
pub mod tool_activity;
