//! Favicons in front of external links in chat markdown (`MarkdownExternalLink.tsx`): a
//! source-control host's brand mark, the globe for hosts the fork never asks about
//! (`isPublicFaviconHost`) and for icons that failed this session, and otherwise the icon
//! `faviconUrlForOrigin` names, fetched once per host. While it loads the slot stays empty, as
//! an unloaded `<img>` does.
//!
//! Fetched icons are kept on disk under the app's data directory, capped at
//! [`DISK_CAP_BYTES`] by dropping the least recently written. Only the live bridge sets the
//! store up ([`init`]); without it (fixtures, headless tools) nothing is fetched and links keep
//! the empty slot.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{App, Global, RenderImage};

use crate::components::icons::{BrandIcon, IconName};

/// Fetched icons kept on disk at most.
const DISK_CAP_BYTES: u64 = 4 * 1024 * 1024;
/// An icon larger than this is not an icon.
const MAX_ICON_BYTES: usize = 256 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// What a link's favicon slot shows.
#[derive(Clone)]
pub enum LinkFavicon {
    /// A monochrome mark in the link's colour: a brand or the globe.
    Mark(&'static str),
    /// The fetched icon.
    Image(Arc<RenderImage>),
    /// Still loading: the slot keeps its space and draws nothing.
    Loading,
}

enum Phase {
    Fetching,
    Ready(Arc<RenderImage>),
    Failed,
}

/// The per-host icons of this session and where fetched ones are kept.
struct LinkFavicons {
    dir: PathBuf,
    hosts: HashMap<String, Phase>,
}

impl Global for LinkFavicons {}

/// Sets where fetched icons are kept and turns fetching on. Called once at bridge start.
pub fn init(data_dir: &Path, cx: &mut App) {
    let dir = data_dir.join("link-favicons");
    _ = std::fs::create_dir_all(&dir);
    cx.set_global(LinkFavicons { dir, hosts: HashMap::new() });
}

/// The host of an external web link (`resolveExternalWebLinkHost`): http and https only,
/// a protocol-relative link read as https.
pub fn external_link_host(href: &str) -> Option<String> {
    let href = if href.starts_with("//") { format!("https:{href}") } else { href.to_owned() };
    let url = url::Url::parse(&href).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.host_str().filter(|host| !host.is_empty()).map(str::to_owned)
}

/// The slot for `host` this frame. A public host not yet asked about is returned in `wanted`
/// so the caller can [`request`] it once it holds `&mut App`.
pub fn link_favicon(host: &str, wanted: &mut Vec<String>, cx: &App) -> LinkFavicon {
    if let Some(brand) = source_control_brand(host) {
        return LinkFavicon::Mark(brand.path());
    }
    if !is_public_favicon_host(host) {
        return LinkFavicon::Mark(IconName::Globe.path());
    }
    let Some(store) = cx.try_global::<LinkFavicons>() else { return LinkFavicon::Loading };
    match store.hosts.get(host) {
        Some(Phase::Ready(image)) => LinkFavicon::Image(image.clone()),
        Some(Phase::Failed) => LinkFavicon::Mark(IconName::Globe.path()),
        Some(Phase::Fetching) => LinkFavicon::Loading,
        None => {
            wanted.push(host.to_owned());
            LinkFavicon::Loading
        }
    }
}

/// Starts loading the icons of `hosts`: from disk when kept, else over the network.
pub fn request(hosts: Vec<String>, cx: &mut App) {
    if hosts.is_empty() || !cx.has_global::<LinkFavicons>() {
        return;
    }
    if !cx.has_global::<crate::connection::ConnectionRuntime>() {
        return;
    }
    let dir = cx.global::<LinkFavicons>().dir.clone();
    for host in hosts {
        let store = cx.global_mut::<LinkFavicons>();
        if store.hosts.contains_key(&host) {
            continue;
        }
        store.hosts.insert(host.clone(), Phase::Fetching);
        let path = dir.join(format!("{:016x}.img", stable_hash(&host)));
        let (dir, fetch_host) = (dir.clone(), host.clone());
        let task = crate::connection::ConnectionRuntime::global(cx).spawn(
            async move {
                let bytes = match tokio::fs::read(&path).await {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        let bytes = fetch(&fetch_host).await.ok_or_else(|| failed("fetch"))?;
                        _ = tokio::fs::write(&path, &bytes).await;
                        trim_cache(&dir).await;
                        bytes
                    }
                };
                decode(&bytes).ok_or_else(|| failed("decode"))
            },
            cx,
        );
        cx.spawn(async move |cx| {
            let phase = match task.await {
                Ok(image) => Phase::Ready(image),
                Err(_) => Phase::Failed,
            };
            cx.update(|cx| {
                cx.global_mut::<LinkFavicons>().hosts.insert(host, phase);
                cx.refresh_windows();
            });
        })
        .detach();
    }
}

fn failed(stage: &str) -> crate::connection::CommandError {
    crate::connection::CommandError::from(format!("link favicon {stage} failed"))
}

/// `faviconUrlForOrigin`: the 32px icon Google's favicon service has for the host.
async fn fetch(host: &str) -> Option<Vec<u8>> {
    let mut url = url::Url::parse("https://www.google.com/s2/favicons").ok()?;
    url.query_pairs_mut().append_pair("domain", host).append_pair("sz", "32");
    let client = reqwest::Client::builder().timeout(FETCH_TIMEOUT).build().ok()?;
    let response = client.get(url).send().await.ok()?.error_for_status().ok()?;
    let bytes = response.bytes().await.ok()?;
    (bytes.len() <= MAX_ICON_BYTES).then(|| bytes.to_vec())
}

/// Decodes an icon into the BGRA frame GPUI uploads.
fn decode(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let mut rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)])))
}

/// Drops the least recently written icons until the directory fits [`DISK_CAP_BYTES`].
async fn trim_cache(dir: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else { return };
    let mut files = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(meta) = entry.metadata().await
            && meta.is_file()
        {
            files.push((meta.modified().ok(), meta.len(), entry.path()));
        }
    }
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    files.sort_by_key(|(modified, ..)| *modified);
    for (_, len, path) in files {
        if total <= DISK_CAP_BYTES {
            break;
        }
        if tokio::fs::remove_file(&path).await.is_ok() {
            total -= len;
        }
    }
}

fn stable_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3))
}

/// `sourceControlClients.findByPublicDomain`: the host or a subdomain of a source-control
/// client's `publicHost`.
fn source_control_brand(host: &str) -> Option<BrandIcon> {
    const PUBLIC_HOSTS: [(&str, BrandIcon); 6] = [
        ("github.com", BrandIcon::GitHub),
        ("origin.cursor.com", BrandIcon::CursorOrigin),
        ("gitlab.com", BrandIcon::GitLab),
        ("git.cafe", BrandIcon::GitCafe),
        ("codeberg.org", BrandIcon::Forgejo),
        ("bitbucket.org", BrandIcon::Bitbucket),
    ];
    let host = host.to_ascii_lowercase();
    PUBLIC_HOSTS
        .into_iter()
        .find(|(public, _)| host == *public || host.strip_suffix(public).is_some_and(|rest| rest.ends_with('.')))
        .map(|(_, brand)| brand)
}

/// `normalizeHostname`: lower case, without IPv6 brackets or trailing dots.
fn normalize_hostname(host: &str) -> String {
    host.to_ascii_lowercase().trim_start_matches('[').trim_end_matches(']').trim_end_matches('.').to_owned()
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 169 && b == 254)
        || (a == 198 && (18..=19).contains(&b))
}

fn is_special_purpose_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    is_private_ipv4(ip)
        || a >= 224
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
}

/// A dotted IPv4 address with exactly four decimal parts, as `parseIpv4Address` reads one.
fn parse_ipv4(host: &str) -> Option<Ipv4Addr> {
    let parts: Vec<u8> = host.split('.').map(|part| part.parse().ok()).collect::<Option<_>>()?;
    let [a, b, c, d] = parts[..] else { return None };
    Some(Ipv4Addr::new(a, b, c, d))
}

fn parse_ipv4_mapped(host: &str) -> Option<Ipv4Addr> {
    let suffix = host.strip_prefix("::ffff:")?;
    parse_ipv4(suffix).or_else(|| host.parse::<Ipv6Addr>().ok()?.to_ipv4_mapped())
}

fn ipv6_prefix(ip: &Ipv6Addr, prefix: [u16; 8], length: u32) -> bool {
    let (ip, prefix) = (u128::from(*ip), u128::from(Ipv6Addr::from(prefix)));
    let mask = if length == 0 { 0 } else { u128::MAX << (128 - length) };
    ip & mask == prefix & mask
}

/// `isPrivateNetworkHost`.
fn is_private_network_host(host: &str) -> bool {
    if host == "::"
        || host == "localhost"
        || host == "::1"
        || parse_ipv4(host).is_some_and(|ip| ip.octets()[0] == 127)
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host == "home.arpa"
        || host.ends_with(".home.arpa")
        || (!host.contains('.') && !host.contains(':'))
        || host.ends_with(".ts.net")
    {
        return true;
    }
    if let Some(ip) = parse_ipv4(host).or_else(|| parse_ipv4_mapped(host)) {
        return is_private_ipv4(ip);
    }
    let first = host.split(':').next().unwrap_or_default();
    if !host.contains(':') || first.is_empty() || first.len() > 4 || !first.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    let hextet = u16::from_str_radix(first, 16).unwrap_or(0);
    (hextet & 0xfe00) == 0xfc00 || (hextet & 0xffc0) == 0xfe80
}

/// `isPublicFaviconHost`: hosts whose name may go to a third-party icon service. Private
/// networks, private and test TLDs, internal labels and special-purpose addresses do not.
pub fn is_public_favicon_host(host: &str) -> bool {
    const PRIVATE_TLDS: [&str; 12] =
        [".alt", ".corp", ".example", ".home", ".internal", ".intranet", ".invalid", ".lan", ".localdomain", ".onion", ".private", ".test"];
    if host.ends_with("..") {
        return false;
    }
    let host = normalize_hostname(host);
    if is_private_network_host(&host) {
        return false;
    }
    if PRIVATE_TLDS.iter().any(|suffix| host == suffix[1..] || host.ends_with(suffix)) {
        return false;
    }
    if host.split('.').any(|label| matches!(label, "corp" | "internal" | "intranet")) {
        return false;
    }
    if let Some(ip) = parse_ipv4(&host).or_else(|| parse_ipv4_mapped(&host)) {
        return !is_special_purpose_ipv4(ip);
    }
    if !host.contains(':') {
        return true;
    }
    let Ok(ip) = host.parse::<Ipv6Addr>() else { return false };
    let s = ip.segments();
    if ipv6_prefix(&ip, [0x0064, 0xff9b, 0, 0, 0, 0, 0, 0], 96) {
        let embedded = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
        return !is_special_purpose_ipv4(embedded);
    }
    if s[0] & 0xe000 != 0x2000 {
        return false;
    }
    if ipv6_prefix(&ip, [0x2001, 0, 0, 0, 0, 0, 0, 0], 23) {
        return (s[1] == 1 && s[2..7].iter().all(|p| *p == 0) && [1, 2, 3].contains(&s[7]))
            || ipv6_prefix(&ip, [0x2001, 3, 0, 0, 0, 0, 0, 0], 32)
            || ipv6_prefix(&ip, [0x2001, 4, 0x0112, 0, 0, 0, 0, 0], 48)
            || ipv6_prefix(&ip, [0x2001, 0x20, 0, 0, 0, 0, 0, 0], 28)
            || ipv6_prefix(&ip, [0x2001, 0x30, 0, 0, 0, 0, 0, 0], 28);
    }
    if ipv6_prefix(&ip, [0x2001, 0x0db8, 0, 0, 0, 0, 0, 0], 32) || ipv6_prefix(&ip, [0x2002, 0, 0, 0, 0, 0, 0, 0], 16) {
        return false;
    }
    !(s[0] == 0x3fff && (s[1] & 0xf000) == 0)
}

/// The link favicon's slot, painted over a placeholder in the link's text: `ms-[0.25em]`, the
/// 14px icon, `me-[0.2em]` (an em and an en space hold about that width).
pub const LINK_FAVICON_PLACEHOLDER: &str = "\u{2003}\u{2002}";
