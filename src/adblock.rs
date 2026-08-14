// Aura Shield — request-level ad/tracker blocking plus cosmetic filtering.
//
// Filters use the Adblock Plus syntax (the practical subset: anchors, wildcards,
// separators, type/domain/party options, @@ exceptions, ## element hiding).
// Rules are indexed by their rarest literal token so a request only ever tests a
// handful of candidates instead of the whole list.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

/// Curated base list, compiled in so the shield works before any download.
pub const BASE_LIST: &str = include_str!("../assets/filters/aura-base.txt");

// ---------------------------------------------------------------- resource types
pub const T_DOCUMENT: u32 = 1 << 0;
pub const T_STYLESHEET: u32 = 1 << 1;
pub const T_IMAGE: u32 = 1 << 2;
pub const T_MEDIA: u32 = 1 << 3;
pub const T_FONT: u32 = 1 << 4;
pub const T_SCRIPT: u32 = 1 << 5;
pub const T_XHR: u32 = 1 << 6;
pub const T_WEBSOCKET: u32 = 1 << 7;
pub const T_PING: u32 = 1 << 8;
pub const T_SUBDOC: u32 = 1 << 9;
pub const T_OTHER: u32 = 1 << 10;

/// Maps COREWEBVIEW2_WEB_RESOURCE_CONTEXT to our type bits.
pub fn type_of_context(ctx: i32) -> u32 {
    match ctx {
        1 => T_DOCUMENT,
        2 => T_STYLESHEET,
        3 => T_IMAGE,
        4 => T_MEDIA,
        5 => T_FONT,
        6 => T_SCRIPT,
        7 | 8 => T_XHR,
        11 => T_WEBSOCKET,
        14 => T_PING,
        _ => T_OTHER,
    }
}

fn type_from_name(name: &str) -> Option<u32> {
    Some(match name {
        "document" | "doc" => T_DOCUMENT,
        "stylesheet" | "css" => T_STYLESHEET,
        "image" | "img" => T_IMAGE,
        "media" => T_MEDIA,
        "font" => T_FONT,
        "script" => T_SCRIPT,
        "xmlhttprequest" | "xhr" | "fetch" => T_XHR,
        "websocket" => T_WEBSOCKET,
        "ping" | "beacon" => T_PING,
        "subdocument" | "frame" => T_SUBDOC,
        "object" | "object-subrequest" | "other" | "webrtc" | "media-source" => T_OTHER,
        _ => return None,
    })
}

// ---------------------------------------------------------------- rules
#[derive(Debug, Clone, PartialEq)]
enum Seg {
    Lit(String),
    Wild,
    Sep,
}

#[derive(Debug)]
struct Rule {
    segs: Vec<Seg>,
    anchor_domain: bool,
    anchor_start: bool,
    anchor_end: bool,
    third_party: Option<bool>,
    types: u32,
    types_not: u32,
    domains: Vec<String>,
    domains_not: Vec<String>,
}

fn is_sep_char(c: u8) -> bool {
    // ABP "^": anything but a letter, digit, or one of _ - . %
    !(c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'.' || c == b'%')
}

impl Rule {
    /// Does this rule's pattern match the URL (ignoring options)?
    fn pattern_matches(&self, url: &[u8], (hs, he): (usize, usize)) -> bool {
        if self.anchor_domain {
            // `||` may start at the host or at any domain-label boundary.
            if self.match_from(url, hs, false) {
                return true;
            }
            for i in hs..he {
                if url[i] == b'.' && i + 1 < he && self.match_from(url, i + 1, false) {
                    return true;
                }
            }
            false
        } else if self.anchor_start {
            self.match_from(url, 0, false)
        } else {
            self.match_from(url, 0, true)
        }
    }

    fn match_from(&self, url: &[u8], start: usize, first_free: bool) -> bool {
        let mut pos = start;
        let mut flexible = first_free;
        let n = self.segs.len();
        for (i, seg) in self.segs.iter().enumerate() {
            match seg {
                Seg::Wild => {
                    flexible = true;
                }
                Seg::Sep => {
                    if flexible {
                        // find the next separator at or after pos
                        let mut p = pos;
                        loop {
                            if p >= url.len() {
                                // end of URL counts as a separator
                                pos = p;
                                break;
                            }
                            if is_sep_char(url[p]) {
                                pos = p + 1;
                                break;
                            }
                            p += 1;
                        }
                        flexible = false;
                    } else if pos >= url.len() {
                        // end-of-URL satisfies "^"
                    } else if is_sep_char(url[pos]) {
                        pos += 1;
                    } else {
                        return false;
                    }
                }
                Seg::Lit(lit) => {
                    let b = lit.as_bytes();
                    if flexible {
                        match find(url, b, pos) {
                            Some(p) => {
                                // The tail must still be satisfiable; try this
                                // occurrence and, on failure, the next ones.
                                let mut cand = p;
                                loop {
                                    if self.match_tail(url, cand + b.len(), i + 1) {
                                        return true;
                                    }
                                    match find(url, b, cand + 1) {
                                        Some(q) => cand = q,
                                        None => return false,
                                    }
                                }
                            }
                            None => return false,
                        }
                    } else {
                        if url.len() < pos + b.len() || &url[pos..pos + b.len()] != b {
                            return false;
                        }
                        pos += b.len();
                        flexible = false;
                    }
                }
            }
            if i + 1 == n {
                break;
            }
        }
        if self.anchor_end {
            return pos == url.len();
        }
        true
    }

    /// Matches segments starting at index `from` — used when a flexible literal
    /// has to try several occurrences.
    fn match_tail(&self, url: &[u8], pos: usize, from: usize) -> bool {
        let mut pos = pos;
        let mut flexible = false;
        for (i, seg) in self.segs.iter().enumerate().skip(from) {
            let _ = i;
            match seg {
                Seg::Wild => flexible = true,
                Seg::Sep => {
                    if flexible {
                        let mut p = pos;
                        loop {
                            if p >= url.len() {
                                pos = p;
                                break;
                            }
                            if is_sep_char(url[p]) {
                                pos = p + 1;
                                break;
                            }
                            p += 1;
                        }
                        flexible = false;
                    } else if pos >= url.len() {
                    } else if is_sep_char(url[pos]) {
                        pos += 1;
                    } else {
                        return false;
                    }
                }
                Seg::Lit(lit) => {
                    let b = lit.as_bytes();
                    if flexible {
                        match find(url, b, pos) {
                            Some(p) => {
                                pos = p + b.len();
                                flexible = false;
                            }
                            None => return false,
                        }
                    } else {
                        if url.len() < pos + b.len() || &url[pos..pos + b.len()] != b {
                            return false;
                        }
                        pos += b.len();
                    }
                }
            }
        }
        if self.anchor_end {
            return pos == url.len();
        }
        true
    }

    /// `||some.host^` with no options — matches a domain and its subdomains.
    fn is_plain_host(&self) -> bool {
        self.anchor_domain
            && !self.anchor_end
            && self.types == 0
            && self.types_not == 0
            && self.third_party.is_none()
            && self.domains.is_empty()
            && self.domains_not.is_empty()
            && self.segs.len() == 2
            && matches!(self.segs[1], Seg::Sep)
            && match &self.segs[0] {
                Seg::Lit(h) => {
                    !h.is_empty()
                        && h.contains('.')
                        && h
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
                }
                _ => false,
            }
    }

    fn options_match(&self, ty: u32, third: bool, doc_host: &str) -> bool {
        if self.types != 0 && self.types & ty == 0 {
            return false;
        }
        if self.types_not & ty != 0 {
            return false;
        }
        if let Some(tp) = self.third_party {
            if tp != third {
                return false;
            }
        }
        if !self.domains.is_empty() && !self.domains.iter().any(|d| host_matches(doc_host, d)) {
            return false;
        }
        if self.domains_not.iter().any(|d| host_matches(doc_host, d)) {
            return false;
        }
        true
    }
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let n = needle.len();
    if n == 0 {
        return Some(from);
    }
    if from + n > hay.len() {
        return None;
    }
    let first = needle[0];
    let last = hay.len() - n;
    let mut i = from;
    while i <= last {
        if hay[i] == first && &hay[i..i + n] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------- token index
/// FNV-1a over the token bytes; the map then hashes the u64 with a cheap mix.
fn tok_hash(s: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in s {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

#[derive(Default)]
pub struct FastHasher(u64);

impl std::hash::Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x1000_0000_01b3);
        }
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = n.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (n >> 29);
    }
}

type TokMap = HashMap<u64, Vec<u32>, std::hash::BuildHasherDefault<FastHasher>>;

/// `host` equals `pattern` or is a subdomain of it.
fn host_matches(host: &str, pattern: &str) -> bool {
    if host == pattern {
        return true;
    }
    host.len() > pattern.len()
        && host.ends_with(pattern)
        && host.as_bytes()[host.len() - pattern.len() - 1] == b'.'
}

const MULTI_TLDS: &[&str] = &[
    "co.uk", "org.uk", "gov.uk", "ac.uk", "co.jp", "or.jp", "ne.jp", "com.au", "net.au",
    "org.au", "com.br", "com.cn", "com.mx", "com.tr", "co.nz", "co.za", "co.in", "com.ar",
    "com.sg", "com.hk", "com.tw",
];

/// Registrable domain via a small public-suffix approximation.
pub fn base_domain(host: &str) -> &str {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() <= 2 {
        return host;
    }
    let last2 = &host[host.len() - (parts[parts.len() - 2].len() + 1 + parts[parts.len() - 1].len())..];
    if MULTI_TLDS.contains(&last2) && parts.len() >= 3 {
        let take = parts[parts.len() - 3].len() + 1 + last2.len();
        return &host[host.len() - take..];
    }
    last2
}

pub fn host_of_url(url: &str) -> &str {
    let rest = match url.find("://") {
        Some(i) => &url[i + 3..],
        None => url,
    };
    let end = rest
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    let auth = &rest[..end];
    let auth = match auth.rfind('@') {
        Some(i) => &auth[i + 1..],
        None => auth,
    };
    match auth.rfind(':') {
        Some(i) if !auth[i + 1..].contains(']') => &auth[..i],
        _ => auth,
    }
}

// ---------------------------------------------------------------- engine
#[derive(Default)]
pub struct Engine {
    pub enabled: bool,
    block: Vec<Rule>,
    allow: Vec<Rule>,
    block_idx: TokMap,
    block_any: Vec<u32>,
    /// Plain `||host^` rules — by far the largest group. Kept as a hash set of
    /// host names so they cost a handful of lookups instead of pattern tests.
    block_hosts: HashSet<u64, std::hash::BuildHasherDefault<FastHasher>>,
    block_host_count: usize,
    allow_idx: TokMap,
    allow_any: Vec<u32>,
    /// Candidate tokens per rule, only alive while lists are being parsed.
    pending: [Vec<Vec<u64>>; 2],
    generic_hide: Vec<String>,
    domain_hide: HashMap<String, Vec<String>>,
    hide_except: HashMap<String, HashSet<String>>,
    allowlist: HashSet<String>,
    /// Sites running in the hardened sandbox (no third-party anything).
    strict: HashSet<String>,
    tab_host: HashMap<u32, String>,
    tab_blocked: HashMap<u32, u32>,
    pub total: u64,
    pub lists: Vec<(String, usize)>,
}

impl Engine {
    pub fn rule_count(&self) -> usize {
        self.block.len() + self.allow.len() + self.block_host_count
    }
    pub fn cosmetic_count(&self) -> usize {
        self.generic_hide.len() + self.domain_hide.values().map(|v| v.len()).sum::<usize>()
    }
}

thread_local! {
    static ENGINE: RefCell<Engine> = RefCell::new(Engine { enabled: true, ..Default::default() });
}

pub fn with_engine<R>(f: impl FnOnce(&mut Engine) -> R) -> R {
    ENGINE.with(|e| f(&mut e.borrow_mut()))
}

// ---------------------------------------------------------------- parsing
fn add_line(e: &mut Engine, line: &str, allow_generic_cosmetic: bool) -> bool {
    let line = line.trim();
    if line.is_empty() || line.starts_with('!') || line.starts_with('[') || line.starts_with("# ") {
        return false;
    }

    // ---- cosmetic rules
    if let Some(idx) = cosmetic_marker(line) {
        let (marker_len, exception) = idx;
        let at = line.find(if exception { "#@#" } else { "##" }).unwrap();
        let doms = &line[..at];
        let sel = line[at + marker_len..].trim();
        if sel.is_empty() || is_procedural(sel) {
            return false;
        }
        if doms.is_empty() {
            if allow_generic_cosmetic && !exception {
                e.generic_hide.push(sel.to_string());
                return true;
            }
            return false;
        }
        let mut any = false;
        for d in doms.split(',') {
            let d = d.trim().trim_start_matches('~').to_ascii_lowercase();
            if d.is_empty() {
                continue;
            }
            if exception {
                e.hide_except.entry(d).or_default().insert(sel.to_string());
            } else {
                e.domain_hide.entry(d).or_default().push(sel.to_string());
            }
            any = true;
        }
        return any;
    }

    // ---- network rules
    let (body, exception) = match line.strip_prefix("@@") {
        Some(rest) => (rest, true),
        None => (line, false),
    };
    let (pattern, opts) = split_options(body);
    if pattern.is_empty() {
        return false;
    }
    // Regex rules are not supported; skipping is safer than guessing.
    if pattern.starts_with('/') && pattern.ends_with('/') && pattern.len() > 2 {
        return false;
    }

    let mut rule = Rule {
        segs: vec![],
        anchor_domain: false,
        anchor_start: false,
        anchor_end: false,
        third_party: None,
        types: 0,
        types_not: 0,
        domains: vec![],
        domains_not: vec![],
    };

    if let Some(opts) = opts {
        for opt in opts.split(',') {
            let (neg, name) = match opt.strip_prefix('~') {
                Some(n) => (true, n),
                None => (false, opt),
            };
            let name = name.trim();
            if let Some(rest) = name.strip_prefix("domain=") {
                for d in rest.split('|') {
                    let d = d.trim().to_ascii_lowercase();
                    if let Some(x) = d.strip_prefix('~') {
                        rule.domains_not.push(x.to_string());
                    } else if !d.is_empty() {
                        rule.domains.push(d);
                    }
                }
            } else if name == "third-party" || name == "3p" {
                rule.third_party = Some(!neg);
            } else if name == "first-party" || name == "1p" {
                rule.third_party = Some(neg);
            } else if name == "popup" {
                rule.types |= T_DOCUMENT | T_SUBDOC;
            } else if name == "all" || name == "important" || name == "match-case" {
                // accepted, no extra behaviour
            } else if let Some(t) = type_from_name(name) {
                if neg {
                    rule.types_not |= t;
                } else {
                    rule.types |= t;
                }
            } else {
                // csp=, redirect=, removeparam=, replace=, badfilter, … — bail out
                // rather than block something the option was meant to narrow.
                return false;
            }
        }
    }

    let mut p = pattern;
    if let Some(rest) = p.strip_prefix("||") {
        rule.anchor_domain = true;
        p = rest;
    } else if let Some(rest) = p.strip_prefix('|') {
        rule.anchor_start = true;
        p = rest;
    }
    if let Some(rest) = p.strip_suffix('|') {
        rule.anchor_end = true;
        p = rest;
    }
    rule.segs = segments(&p.to_ascii_lowercase());
    if rule.segs.is_empty() {
        return false;
    }

    // Fast path: an unqualified `||host^` rule is just "block this domain".
    if !exception && rule.is_plain_host() {
        if let Seg::Lit(h) = &rule.segs[0] {
            e.block_hosts.insert(tok_hash(h.as_bytes()));
            e.block_host_count += 1;
            return true;
        }
    }

    let tokens = candidate_tokens(&rule);
    let slot = usize::from(exception);
    let vec = if exception { &mut e.allow } else { &mut e.block };
    vec.push(rule);
    e.pending[slot].push(tokens);
    true
}

/// Assigns every rule to its *rarest* candidate token, so no bucket collects
/// thousands of rules behind a common word like "track" or "banner".
fn finalize(e: &mut Engine) {
    for slot in 0..2 {
        let pending = std::mem::take(&mut e.pending[slot]);
        let mut freq: HashMap<u64, u32, std::hash::BuildHasherDefault<FastHasher>> =
            HashMap::with_capacity_and_hasher(pending.len() * 2, Default::default());
        for toks in &pending {
            for t in toks {
                *freq.entry(*t).or_insert(0) += 1;
            }
        }
        let (idx, any) = if slot == 1 {
            (&mut e.allow_idx, &mut e.allow_any)
        } else {
            (&mut e.block_idx, &mut e.block_any)
        };
        for (i, toks) in pending.iter().enumerate() {
            match toks.iter().min_by_key(|t| freq[*t]) {
                Some(t) => idx.entry(*t).or_default().push(i as u32),
                None => any.push(i as u32),
            }
        }
    }
}

/// Returns (marker length, is_exception) for element-hiding rules.
fn cosmetic_marker(line: &str) -> Option<(usize, bool)> {
    if let Some(i) = line.find("#@#") {
        let _ = i;
        return Some((3, true));
    }
    if let Some(i) = line.find("##") {
        // "##" inside a network pattern is not a thing, but a leading "#" comment is.
        let _ = i;
        return Some((2, false));
    }
    None
}

fn is_procedural(sel: &str) -> bool {
    sel.contains(":has(")
        || sel.contains(":has-text(")
        || sel.contains(":-abp-")
        || sel.contains(":matches-")
        || sel.contains(":upward(")
        || sel.contains(":remove(")
        || sel.contains(":style(")
        || sel.contains(":xpath(")
        || sel.contains(":watch-attr(")
        || sel.contains(":others(")
        || sel.starts_with('+')
        || sel.starts_with('^')
}

/// Splits "pattern$opt,opt" — only when the tail really looks like an option list.
fn split_options(s: &str) -> (&str, Option<&str>) {
    if let Some(i) = s.rfind('$') {
        let tail = &s[i + 1..];
        if !tail.is_empty()
            && tail.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, ',' | '~' | '=' | '|' | '-' | '_' | '.' | '/' | ':' | '*')
            })
        {
            return (&s[..i], Some(tail));
        }
    }
    (s, None)
}

fn segments(p: &str) -> Vec<Seg> {
    let mut out = vec![];
    let mut cur = String::new();
    for c in p.chars() {
        match c {
            '*' => {
                if !cur.is_empty() {
                    out.push(Seg::Lit(std::mem::take(&mut cur)));
                }
                if out.last() != Some(&Seg::Wild) {
                    out.push(Seg::Wild);
                }
            }
            '^' => {
                if !cur.is_empty() {
                    out.push(Seg::Lit(std::mem::take(&mut cur)));
                }
                out.push(Seg::Sep);
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(Seg::Lit(cur));
    }
    out
}

/// Every literal run that is guaranteed to show up as a whole token in a URL.
/// A run only qualifies when both of its edges are hard boundaries in the
/// pattern, otherwise a wildcard could glue it to more characters in the URL.
fn candidate_tokens(r: &Rule) -> Vec<u64> {
    let mut out = Vec::new();
    let n = r.segs.len();
    for (i, seg) in r.segs.iter().enumerate() {
        let Seg::Lit(lit) = seg else { continue };
        let b = lit.as_bytes();
        let mut s = 0usize;
        while s < b.len() {
            if !b[s].is_ascii_alphanumeric() {
                s += 1;
                continue;
            }
            let mut e = s;
            while e < b.len() && b[e].is_ascii_alphanumeric() {
                e += 1;
            }
            let left_ok = s > 0 || (i == 0 && (r.anchor_domain || r.anchor_start));
            let right_ok = e < b.len()
                || (i + 1 < n && matches!(r.segs[i + 1], Seg::Sep))
                || (i + 1 == n && r.anchor_end);
            if left_ok && right_ok && e - s >= 3 {
                out.push(tok_hash(&b[s..e]));
            }
            s = e;
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

const MAX_URL_TOKENS: usize = 32;

/// Maximal alphanumeric runs of the URL, hashed, capped for pathological URLs.
fn url_tokens(url: &[u8], out: &mut [u64; MAX_URL_TOKENS]) -> usize {
    let mut n = 0usize;
    let mut s = 0usize;
    while s < url.len() && n < MAX_URL_TOKENS {
        if !url[s].is_ascii_alphanumeric() {
            s += 1;
            continue;
        }
        let mut e = s;
        while e < url.len() && url[e].is_ascii_alphanumeric() {
            e += 1;
        }
        if e - s >= 3 {
            out[n] = tok_hash(&url[s..e]);
            n += 1;
        }
        s = e;
    }
    n
}

// ---------------------------------------------------------------- public API
pub fn load(base: &str, extra: &[(String, String)], enabled: bool) {
    with_engine(|e| {
        let keep_allow = std::mem::take(&mut e.allowlist);
        let keep_strict = std::mem::take(&mut e.strict);
        let keep_total = e.total;
        let keep_hosts = std::mem::take(&mut e.tab_host);
        *e = Engine { enabled, ..Default::default() };
        e.allowlist = keep_allow;
        e.strict = keep_strict;
        e.total = keep_total;
        e.tab_host = keep_hosts;

        let mut n = 0usize;
        for line in base.lines() {
            if add_line(e, line, true) {
                n += 1;
            }
        }
        e.lists.push(("Aura Basis".to_string(), n));

        for (name, text) in extra {
            let mut n = 0usize;
            for line in text.lines() {
                if add_line(e, line, false) {
                    n += 1;
                }
            }
            e.lists.push((name.clone(), n));
        }
        finalize(e);
    });
}

/// Engine built on a worker thread, waiting to be installed on the UI thread.
static PENDING: std::sync::Mutex<Option<Engine>> = std::sync::Mutex::new(None);

/// Parses the cached lists off the UI thread and posts `msg` when they are ready.
/// Startup only pays for the (tiny) built-in list.
pub fn load_cached_async(dir: std::path::PathBuf, enabled: bool, hwnd: isize, msg: u32) {
    std::thread::spawn(move || {
        let cached = read_cached(&dir);
        if cached.is_empty() {
            return;
        }
        let mut e = Engine { enabled, ..Default::default() };
        let mut n = 0usize;
        for line in BASE_LIST.lines() {
            if add_line(&mut e, line, true) {
                n += 1;
            }
        }
        e.lists.push(("Aura Basis".to_string(), n));
        for (name, text) in &cached {
            let mut n = 0usize;
            for line in text.lines() {
                if add_line(&mut e, line, false) {
                    n += 1;
                }
            }
            e.lists.push((name.clone(), n));
        }
        finalize(&mut e);
        if let Ok(mut slot) = PENDING.lock() {
            *slot = Some(e);
        }
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void)),
                msg,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
            );
        }
    });
}

/// Installs a background-parsed engine, keeping live session state.
pub fn install_pending() -> bool {
    let Some(mut fresh) = PENDING.lock().ok().and_then(|mut s| s.take()) else {
        return false;
    };
    with_engine(|e| {
        fresh.enabled = e.enabled;
        fresh.allowlist = std::mem::take(&mut e.allowlist);
        fresh.strict = std::mem::take(&mut e.strict);
        fresh.tab_host = std::mem::take(&mut e.tab_host);
        fresh.tab_blocked = std::mem::take(&mut e.tab_blocked);
        fresh.total = e.total;
        *e = fresh;
    });
    true
}

pub fn set_enabled(on: bool) {
    with_engine(|e| e.enabled = on);
}

thread_local! {
    static DNT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static HTTPS_ONLY: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    static HTTP_OK: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// Whether outgoing requests get DNT / Sec-GPC headers.
pub fn send_dnt() -> bool {
    DNT.with(|d| d.get())
}

pub fn set_dnt(on: bool) {
    DNT.with(|d| d.set(on));
}

thread_local! {
    static STRICT_POPUPS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Only links may open windows (kills popunders, may break payment popups).
pub fn strict_popups() -> bool {
    STRICT_POPUPS.with(|d| d.get())
}

pub fn set_strict_popups(on: bool) {
    STRICT_POPUPS.with(|d| d.set(on));
}

/// Would this URL be blocked as a top-level document (popup/popunder target)?
pub fn is_blocked_popup(tab: u32, url: &str) -> bool {
    should_block(tab, url, 1) // COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT
}

/// Upgrade plain http:// navigations to https://.
pub fn https_only() -> bool {
    HTTPS_ONLY.with(|d| d.get())
}

pub fn set_https_only(on: bool) {
    HTTPS_ONLY.with(|d| d.set(on));
}

/// Hosts the user explicitly allowed to stay on plain HTTP.
pub fn http_allowed(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    HTTP_OK.with(|s| s.borrow().contains(&h))
}

pub fn allow_http(host: &str) {
    HTTP_OK.with(|s| s.borrow_mut().insert(host.to_ascii_lowercase()));
}

pub fn is_enabled() -> bool {
    with_engine(|e| e.enabled)
}

pub fn set_allowlist(hosts: &[String]) {
    with_engine(|e| e.allowlist = hosts.iter().map(|h| h.to_ascii_lowercase()).collect());
}

pub fn allowlist() -> Vec<String> {
    with_engine(|e| {
        let mut v: Vec<String> = e.allowlist.iter().cloned().collect();
        v.sort();
        v
    })
}

pub fn is_allowlisted(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    with_engine(|e| e.allowlist.iter().any(|d| host_matches(&h, d)))
}

// ---------------------------------------------------------------- strict mode
pub fn set_strict_sites(hosts: &[String]) {
    with_engine(|e| e.strict = hosts.iter().map(|h| h.to_ascii_lowercase()).collect());
}

pub fn strict_sites() -> Vec<String> {
    with_engine(|e| {
        let mut v: Vec<String> = e.strict.iter().cloned().collect();
        v.sort();
        v
    })
}

/// Is this host sandboxed (all third-party traffic, downloads and permissions off)?
pub fn is_strict(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    with_engine(|e| e.strict.iter().any(|d| host_matches(&h, d)))
}

/// Toggles the hardened mode for a site; returns the new state.
pub fn toggle_strict(host: &str) -> bool {
    let h = base_domain(&host.to_ascii_lowercase()).to_string();
    with_engine(|e| {
        if e.strict.contains(&h) {
            e.strict.remove(&h);
            false
        } else {
            e.strict.insert(h);
            true
        }
    })
}

/// True when the tab's current site runs hardened.
pub fn tab_is_strict(tab: u32) -> bool {
    with_engine(|e| match e.tab_host.get(&tab) {
        Some(h) => e.strict.iter().any(|d| host_matches(h, d)),
        None => false,
    })
}

pub fn toggle_site(host: &str) -> bool {
    let h = base_domain(&host.to_ascii_lowercase()).to_string();
    with_engine(|e| {
        if e.allowlist.contains(&h) {
            e.allowlist.remove(&h);
            false
        } else {
            e.allowlist.insert(h);
            true
        }
    })
}

pub fn set_tab_host(tab: u32, url: &str) {
    let h = host_of_url(url).to_ascii_lowercase();
    with_engine(|e| {
        e.tab_host.insert(tab, h);
        e.tab_blocked.insert(tab, 0);
    });
}

pub fn forget_tab(tab: u32) {
    with_engine(|e| {
        e.tab_host.remove(&tab);
        e.tab_blocked.remove(&tab);
    });
}

pub fn blocked_for(tab: u32) -> u32 {
    with_engine(|e| e.tab_blocked.get(&tab).copied().unwrap_or(0))
}

pub fn total_blocked() -> u64 {
    with_engine(|e| e.total)
}

pub fn set_total(v: u64) {
    with_engine(|e| e.total = v);
}

pub fn stats() -> (u64, usize, usize, Vec<(String, usize)>) {
    with_engine(|e| (e.total, e.rule_count(), e.cosmetic_count(), e.lists.clone()))
}

/// The hot path: is this sub-resource request an ad/tracker?
pub fn should_block(tab: u32, url: &str, ctx: i32) -> bool {
    if url.len() > 2048 {
        return false;
    }
    // URLs are almost always lowercase already — only copy when they are not.
    let lower: std::borrow::Cow<str> = if url.bytes().any(|b| b.is_ascii_uppercase()) {
        std::borrow::Cow::Owned(url.to_ascii_lowercase())
    } else {
        std::borrow::Cow::Borrowed(url)
    };
    let lower = lower.as_ref();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return false;
    }
    let ty = type_of_context(ctx);

    let blocked = ENGINE.with(|cell| {
        let e = cell.borrow();
        if !e.enabled {
            return false;
        }
        let Some(doc_host) = e.tab_host.get(&tab) else { return false };
        if doc_host.is_empty() || doc_host == "aura.internal" {
            return false;
        }
        if e.allowlist.iter().any(|d| host_matches(doc_host, d)) {
            return false;
        }
        // Host span inside the URL, without searching.
        let scheme = if lower.as_bytes()[4] == b's' { 8 } else { 7 };
        let rest = &lower[scheme..];
        let host_len = rest
            .find(['/', '?', '#'])
            .unwrap_or(rest.len());
        let host = &rest[..host_len];
        let host = match host.rfind('@') {
            Some(i) => &host[i + 1..],
            None => host,
        };
        let host = match host.rfind(':') {
            Some(i) => &host[..i],
            None => host,
        };
        if host == "aura.internal" {
            return false;
        }
        let hs = scheme + (host.as_ptr() as usize - rest.as_ptr() as usize);
        let span = (hs, hs + host.len());
        let third = base_domain(host) != base_domain(doc_host);
        // Hardened sites talk to nobody but themselves.
        if third && e.strict.iter().any(|d| host_matches(doc_host, d)) {
            return true;
        }
        let bytes = lower.as_bytes();
        let mut toks = [0u64; MAX_URL_TOKENS];
        let ntok = url_tokens(bytes, &mut toks);
        let toks = &toks[..ntok];

        // Exceptions win, so test them first.
        if hits(&e.allow, &e.allow_idx, &e.allow_any, toks, bytes, span, ty, third, doc_host) {
            return false;
        }
        // Blocked domain? Check the host and each of its parents.
        let mut probe = host;
        loop {
            if e.block_hosts.contains(&tok_hash(probe.as_bytes())) {
                return true;
            }
            match probe.find('.') {
                Some(i) if probe[i + 1..].contains('.') => probe = &probe[i + 1..],
                _ => break,
            }
        }
        hits(&e.block, &e.block_idx, &e.block_any, toks, bytes, span, ty, third, doc_host)
    });

    if blocked {
        ENGINE.with(|cell| {
            let mut e = cell.borrow_mut();
            e.total += 1;
            *e.tab_blocked.entry(tab).or_insert(0) += 1;
        });
    }
    blocked
}

#[allow(clippy::too_many_arguments)]
fn hits(
    rules: &[Rule],
    idx: &TokMap,
    any: &[u32],
    tokens: &[u64],
    url: &[u8],
    span: (usize, usize),
    ty: u32,
    third: bool,
    doc_host: &str,
) -> bool {
    let test = |id: u32| -> bool {
        let r = &rules[id as usize];
        r.options_match(ty, third, doc_host) && r.pattern_matches(url, span)
    };
    for t in tokens {
        if let Some(bucket) = idx.get(t) {
            for id in bucket {
                if test(*id) {
                    return true;
                }
            }
        }
    }
    for id in any {
        if test(*id) {
            return true;
        }
    }
    false
}

/// CSS that hides leftover ad containers on `host`.
pub fn cosmetic_css(host: &str) -> String {
    let host = host.to_ascii_lowercase();
    if host.is_empty() || host == "aura.internal" || is_allowlisted(&host) {
        return String::new();
    }
    with_engine(|e| {
        if !e.enabled {
            return String::new();
        }
        let mut except: HashSet<&str> = HashSet::new();
        let mut sels: Vec<&str> = e.generic_hide.iter().map(|s| s.as_str()).collect();
        let mut probe = host.as_str();
        loop {
            if let Some(v) = e.domain_hide.get(probe) {
                sels.extend(v.iter().map(|s| s.as_str()));
            }
            if let Some(v) = e.hide_except.get(probe) {
                except.extend(v.iter().map(|s| s.as_str()));
            }
            match probe.find('.') {
                Some(i) if probe[i + 1..].contains('.') => probe = &probe[i + 1..],
                _ => break,
            }
        }
        sels.retain(|s| !except.contains(s));
        sels.sort_unstable();
        sels.dedup();
        if sels.is_empty() {
            return String::new();
        }
        format!(
            "{}{{display:none!important}}",
            sels.join(",")
        )
    })
}

// ---------------------------------------------------------------- list files
pub const LIST_SOURCES: &[(&str, &str)] = &[
    ("easylist", "https://easylist.to/easylist/easylist.txt"),
    ("easyprivacy", "https://easylist.to/easylist/easyprivacy.txt"),
    ("easylist-de", "https://easylist.to/easylistgermany/easylistgermany.txt"),
    ("ublock-filters", "https://ublockorigin.github.io/uAssets/filters/filters.txt"),
    ("ublock-privacy", "https://ublockorigin.github.io/uAssets/filters/privacy.txt"),
    ("ublock-badware", "https://ublockorigin.github.io/uAssets/filters/badware.txt"),
    ("adservers", "https://pgl.yoyo.org/adservers/serverlist.php?hostformat=adblockplus&showintro=0&mimetype=plaintext"),
];

pub fn filters_dir(profile_dir: &std::path::Path) -> std::path::PathBuf {
    profile_dir.join("filters")
}

/// Reads every cached list from disk (name, contents).
pub fn read_cached(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let name = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
        if let Ok(text) = std::fs::read_to_string(&p) {
            out.push((name, text));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Age of the cache in seconds (u64::MAX when there is none).
pub fn cache_age(dir: &std::path::Path) -> u64 {
    let mut newest: Option<std::time::SystemTime> = None;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            if let Ok(m) = entry.metadata() {
                if let Ok(t) = m.modified() {
                    if newest.map(|n| t > n).unwrap_or(true) {
                        newest = Some(t);
                    }
                }
            }
        }
    }
    match newest.and_then(|t| t.elapsed().ok()) {
        Some(d) => d.as_secs(),
        None => u64::MAX,
    }
}

/// Downloads all sources in the background; posts `msg` to `hwnd` when done.
/// Uses the curl.exe that ships with Windows so we don't need an HTTP stack.
pub fn update_async(dir: std::path::PathBuf, hwnd: isize, msg: u32) {
    std::thread::spawn(move || {
        let _ = std::fs::create_dir_all(&dir);
        let mut any = false;
        for (name, url) in LIST_SOURCES {
            let tmp = dir.join(format!("{name}.part"));
            let out = dir.join(format!("{name}.txt"));
            let ok = run_curl(url, &tmp);
            if ok && std::fs::metadata(&tmp).map(|m| m.len() > 4096).unwrap_or(false) {
                let _ = std::fs::rename(&tmp, &out);
                any = true;
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
        }
        if any {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void)),
                    msg,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn engine_with_all_lists() -> (usize, u128) {
        let dir = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default())
            .join("AuraBrowser")
            .join("Test")
            .join("filters");
        let cached = if dir.is_dir() { read_cached(&dir) } else { vec![] };
        let t0 = Instant::now();
        load(BASE_LIST, &cached, true);
        let ms = t0.elapsed().as_millis();
        (with_engine(|e| e.rule_count()), ms)
    }

    #[test]
    fn parses_and_matches() {
        let (rules, ms) = engine_with_all_lists();
        println!("parsed {rules} network rules in {ms} ms");
        assert!(rules > 200, "base list alone should yield >200 rules");

        set_tab_host(1, "https://www.heise.de/news/foo.html");

        // Ads and trackers go.
        for u in [
            "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
            "https://ad.doubleclick.net/ddm/trackimp",
            "https://www.google-analytics.com/analytics.js",
            "https://cdn.taboola.com/libtrc/loader.js",
            "https://static.criteo.net/js/ld/ld.js",
        ] {
            assert!(should_block(1, u, 6), "should have blocked {u}");
        }

        // First-party content and payment/captcha infrastructure stay.
        for u in [
            "https://www.heise.de/assets/main.js",
            "https://heise.cloudimg.io/v7/_www-heise-de_/imgs/photo.jpg",
            "https://js.stripe.com/v3/",
            "https://www.google.com/recaptcha/api.js",
        ] {
            assert!(!should_block(1, u, 6), "should NOT have blocked {u}");
        }
    }

    #[test]
    fn domain_anchor_respects_label_boundaries() {
        load("||doubleclick.net^\n", &[], true);
        set_tab_host(2, "https://example.org/");
        assert!(should_block(2, "https://ad.doubleclick.net/x", 3));
        assert!(should_block(2, "https://doubleclick.net/x", 3));
        assert!(!should_block(2, "https://notdoubleclick.net.evil.test/x", 3));
    }

    #[test]
    fn index_quality() {
        engine_with_all_lists();
        with_engine(|e| {
            let buckets = e.block_idx.len();
            let biggest = e.block_idx.values().map(|v| v.len()).max().unwrap_or(0);
            println!(
                "block: {} rules, {} untokenized, {} buckets, biggest {}",
                e.block.len(),
                e.block_any.len(),
                buckets,
                biggest
            );
            println!("allow: {} rules, {} untokenized", e.allow.len(), e.allow_any.len());
            // Only meaningful once the downloaded lists are present; with the
            // built-in list alone the sample is far too small to say anything.
            if e.block.len() > 5000 {
                assert!(
                    e.block_any.len() * 20 < e.block.len(),
                    "too many untokenized rules: {} of {}",
                    e.block_any.len(),
                    e.block.len()
                );
            } else {
                println!("  (nur Basisliste geladen – Verhältnis wird nicht geprüft)");
            }
        });
    }

    #[test]
    fn match_speed() {
        let (_, _) = engine_with_all_lists();
        set_tab_host(3, "https://www.heise.de/");
        let urls = [
            "https://www.heise.de/assets/app.7fd1.js",
            "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
            "https://heise.cloudimg.io/v7/img/hero.webp",
            "https://cdn.taboola.com/libtrc/loader.js",
            "https://www.heise.de/api/comments?id=42",
        ];
        let t0 = Instant::now();
        let n = 20_000;
        for i in 0..n {
            should_block(3, urls[i % urls.len()], 6);
        }
        let per = t0.elapsed().as_nanos() / n as u128;
        println!("{per} ns per lookup");
        assert!(per < 200_000, "lookup too slow: {per} ns");
    }

    #[test]
    fn cosmetic_rules_are_scoped() {
        load("##.adsbygoogle\nheise.de##.werbung\nspiegel.de##.anzeige\n", &[], true);
        let css = cosmetic_css("www.heise.de");
        assert!(css.contains(".adsbygoogle"));
        assert!(css.contains(".werbung"));
        assert!(!css.contains(".anzeige"));
    }
}

fn run_curl(url: &str, out: &std::path::Path) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("curl.exe")
        .args([
            "-sSL",
            "--max-time",
            "45",
            "--retry",
            "1",
            "-A",
            "Aura Browser Shield",
            "-o",
        ])
        .arg(out)
        .arg(url)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
