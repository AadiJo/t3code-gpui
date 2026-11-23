//! The chat view: header, timeline and markdown. Ports `components/chat`.

pub mod chat_header;
pub mod chat_icons;
pub mod chat_style;
pub mod markdown_stream_blocks;
pub mod messages_timeline;
pub mod messages_timeline_logic;
pub mod pending_user_input_panel;
pub mod rich_text;
pub mod syntax_highlighting;
pub mod syntax_theme;
mod syntax_theme_data;
mod timeline_assistant_rows;
pub mod timeline_entries;
mod timeline_event_rows;
pub mod timeline_fixtures;
pub mod timeline_row_parts;
mod timeline_user_message_row;
pub mod timeline_viewport;
pub mod virtual_list;
mod timeline_work_entry_row;
mod timeline_work_group_rows;
pub mod work_log_presentation;
