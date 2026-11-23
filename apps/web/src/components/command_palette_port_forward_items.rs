//! The Forwarded ports page. Ports `CommandPalettePortForwardItems.tsx`:
//! forwarded ports with open, copy and stop actions, servers to forward,
//! and a row that forwards a port number typed into the query.

use std::rc::Rc;

use super::command_palette_logic::{
    PaletteEvent, PaletteGroup, PaletteItem, PaletteRun, PaletteTitle, PaletteView, PortForwardRequest,
};
use super::command_palette_lookups::PalettePort;
use super::icons::IconName;
use crate::i18n::t;

fn request(event: PortForwardRequest) -> PaletteRun {
    PaletteRun::Event(PaletteEvent::PortForward(event))
}

/// The Forwarded ports row and its page.
pub fn port_forwards_item(ports: &[PalettePort]) -> PaletteItem {
    let forwarded: Vec<Rc<PaletteItem>> = ports
        .iter()
        .filter_map(|port| {
            let local = port.local_port?;
            let (environment_id, remote) = (port.environment_id.clone(), port.remote_port);
            let address = format!("localhost:{local}");
            let actions = vec![
                (t!("Open in browser"), IconName::ExternalLink, PortForwardRequest::Open { environment_id: environment_id.clone(), port: remote }, "port-forward:open"),
                (t!("Copy address"), IconName::ClipboardCopy, PortForwardRequest::CopyAddress { environment_id: environment_id.clone(), port: remote }, "port-forward:copy"),
                (t!("Stop forwarding"), IconName::Square, PortForwardRequest::Stop { environment_id: environment_id.clone(), port: remote }, "port-forward:stop"),
            ]
            .into_iter()
            .map(|(title, icon, event, value)| {
                let mut item = PaletteItem::action(value, PaletteTitle::Text(title.clone()), icon, request(event), &[]);
                item.set_terms([title]);
                Rc::new(item)
            })
            .collect();
            let view = PaletteView {
                addon_icon: IconName::Cable,
                groups: vec![PaletteGroup { value: "port-forward-actions".into(), label: t!("Forwarded port {port}", port = remote), items: actions }],
                initial_query: None,
            };
            let mut item = PaletteItem::submenu(
                format!("port-forward:{}:{remote}", environment_id.0),
                address.clone().into(),
                IconName::Cable,
                view,
                &[],
            );
            item.set_terms([address.into(), remote.to_string().into(), port.environment_label.clone()]);
            Some(Rc::new(item.description(t!("Port {port} on {environment}", port = remote, environment = port.environment_label.clone()))))
        })
        .collect();
    let start: Vec<Rc<PaletteItem>> = ports
        .iter()
        .filter(|port| port.local_port.is_none())
        .map(|port| {
            let title = t!("Forward localhost:{port}", port = port.remote_port);
            let event = PortForwardRequest::Start { environment_id: port.environment_id.clone(), port: port.remote_port };
            let mut item = PaletteItem::action(
                format!("port-forward:start:{}:{}", port.environment_id.0, port.remote_port),
                PaletteTitle::Text(title.clone()),
                IconName::Cable,
                request(event),
                &[],
            );
            item.set_terms([title, port.remote_port.to_string().into()]);
            Rc::new(item.description(port.environment_label.clone()))
        })
        .collect();
    let mut groups = Vec::new();
    if !forwarded.is_empty() {
        groups.push(PaletteGroup { value: "port-forwards:forwarded".into(), label: t!("Forwarded"), items: forwarded });
    }
    let start_label = if start.is_empty() { Default::default() } else { t!("Forward a port") };
    let mut start = start;
    if start.is_empty() {
        let mut hint = PaletteItem::action(
            "port-forward:hint",
            PaletteTitle::Text(t!("Type a port number to forward it")),
            IconName::Cable,
            PaletteRun::Event(PaletteEvent::OpenSettings),
            &[],
        );
        hint.disabled = true;
        start.push(Rc::new(hint));
    }
    groups.push(PaletteGroup { value: "port-forwards:start".into(), label: start_label, items: start });
    let view = PaletteView { addon_icon: IconName::Cable, groups, initial_query: None };
    PaletteItem::submenu("action:port-forwards", t!("Forwarded ports"), IconName::Cable, view, &["port", "forward", "tunnel", "localhost"])
}
