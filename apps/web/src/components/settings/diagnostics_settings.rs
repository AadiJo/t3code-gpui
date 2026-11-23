//! Settings → Diagnostics. Ports the parts of `DiagnosticsSettings.tsx` and
//! `TelemetryExportSettings.tsx` that read as tables: OpenTelemetry export
//! URLs, live processes, host resources and trace summaries. The resource
//! history charts (`ResourceTelemetryDiagnostics.tsx`) are not built.

use gpui_kit::{Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};
use t3code_contracts::{ClientServerSettingsPatch, ClientServerSettingsPatchObservability};

use super::server_settings_controls::{Fields, field_input, hint, save_patch};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::i18n::t;

/// The Diagnostics page.
pub struct DiagnosticsSettingsPanel {
    model: Entity<ServerPanelsModel>,
    fields: Fields,
    _observe: Subscription,
}

fn bytes(n: u64) -> String {
    let mb = n as f64 / (1024. * 1024.);
    if mb >= 1024. { format!("{:.1} GB", mb / 1024.) } else { format!("{mb:.0} MB") }
}

impl DiagnosticsSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        model.update(cx, |m, cx| {
            m.run("diagnostics-processes", |s| s.process_diagnostics(), |_, _| {}, cx);
            m.run("diagnostics-traces", |s| s.trace_diagnostics(), |_, _| {}, cx);
            m.run("diagnostics-host", |s| s.host_resources(), |_, _| {}, cx);
        });
        Self { model, fields: Fields::default(), _observe: observe }
    }

    /// The page's nav title.
    pub fn page_title() -> SharedString {
        t!("Diagnostics")
    }
}

impl Render for DiagnosticsSettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let snapshot = self.model.read(cx).snapshot().clone();
        let ro = snapshot.read_only;
        let obs = snapshot.settings.observability.clone().unwrap_or_default();
        let mut export = SettingsSection::new(t!("OpenTelemetry export"));
        for (key, label, value) in [
            ("traces", t!("Traces"), obs.otlp_traces_url.clone()),
            ("metrics", t!("Metrics"), obs.otlp_metrics_url.clone()),
            ("logs", t!("Logs"), obs.otlp_logs_url.clone()),
        ] {
            let field = self.fields.get(format!("otlp:{key}"), value.unwrap_or_default(), "https://otel.example.com/v1/traces", false, window, cx);
            let read = field.clone();
            let model = self.model.clone();
            export.extend([SettingsRow::new(label)
                .description(t!("Enter an HTTP or HTTPS endpoint, or leave empty to disable export."))
                .control(div().flex().items_center().gap(theme.sp(2.)).child(div().w(theme.sp(64.)).child(field_input(&field, true, false, ro))).child(
                    Button::new(SharedString::from(format!("otlp-save-{key}"))).variant(ButtonVariant::Outline).size(ButtonSize::Xs).label(t!("Save")).disabled(ro).on_click(move |_, _, cx| {
                        let url = read.read(cx).value().trim().to_owned();
                        let mut o = ClientServerSettingsPatchObservability::default();
                        match key {
                            "traces" => o.otlp_traces_url = Some(url),
                            "metrics" => o.otlp_metrics_url = Some(url),
                            _ => o.otlp_logs_url = Some(url),
                        }
                        save_patch(&model, "observability", ClientServerSettingsPatch { observability: Some(o), ..Default::default() }, cx);
                    }),
                ))
                .into_any_element()]);
        }

        let mut host = SettingsSection::new(t!("Host"));
        match &snapshot.host_resources {
            Some(h) => host.extend([SettingsRow::new(t!("Memory"))
                .description(SharedString::from(format!(
                    "{} free of {} · {} CPUs",
                    bytes(h.available_memory_bytes),
                    bytes(h.total_memory_bytes),
                    h.cpu_count
                )))
                .into_any_element()]),
            None => host.extend([SettingsRow::new(t!("Loading…")).into_any_element()]),
        }

        let mut processes = SettingsSection::new(t!("Live Processes"));
        match &snapshot.process_diagnostics {
            Some(p) => {
                processes.extend([SettingsRow::new(SharedString::from(format!("{} processes · {}", p.process_count, bytes(p.total_rss_bytes)))).into_any_element()]);
                processes.extend([div()
                    .px(theme.sp(4.))
                    .pb(theme.sp(3.))
                    .flex()
                    .flex_col()
                    .children(p.processes.iter().take(12).map(|proc_| {
                        div()
                            .flex()
                            .gap(theme.sp(3.))
                            .py(px(3.))
                            .mono_text(px(12.), weight::NORMAL)
                            .child(div().w(px(56.)).text_color(c.muted_foreground).child(proc_.pid.to_string()))
                            .child(div().w(px(72.)).child(bytes(proc_.rss_bytes)))
                            .child(div().flex_1().truncate().pl(px(proc_.depth as f32 * 12.)).child(proc_.command.clone()))
                    }))
                    .into_any_element()]);
            }
            None => processes.extend([SettingsRow::new(t!("Loading…")).into_any_element()]),
        }

        let mut traces = SettingsSection::new(t!("Trace Diagnostics"));
        match &snapshot.trace_diagnostics {
            Some(tr) => {
                traces.extend([SettingsRow::new(SharedString::from(format!(
                    "{} spans · {} failures · {} slow",
                    tr.record_count, tr.failure_count, tr.slow_span_count
                )))
                .description(SharedString::from(tr.trace_file_path.clone()))
                .into_any_element()]);
                let json = serde_json::to_value(&tr.common_failures).unwrap_or_default();
                for failure in json.as_array().into_iter().flatten().take(5) {
                    let name = failure.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
                    let count = failure.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
                    traces.extend([SettingsRow::new(SharedString::from(name)).status(SharedString::from(format!("{count}×"))).into_any_element()]);
                }
            }
            None => traces.extend([SettingsRow::new(t!("Loading…")).into_any_element()]),
        }
        page_container(cx).child(export).child(host).child(processes).child(traces)
    }
}
