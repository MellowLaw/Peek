use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::State;

use crate::preferences::PreferencesState;

const CACHE_FILE: &str = "lyrics_cache.json";
const AMLL_INDEX_FILE: &str = "amll_index.jsonl";
const AMLL_MAX_AGE_SECS: u64 = 7 * 24 * 3600;
const LRCLIB_MAX_BYTES: usize = 512 * 1024;
const AMLL_INDEX_MAX_BYTES: usize = 20 * 1024 * 1024;
const AMLL_TTML_MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricWord {
    pub time_seconds: f64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricLine {
    pub time_seconds: f64,
    pub text: String,
    pub words: Vec<LyricWord>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum LyricalSnapshot {
    Disabled,
    Offline,
    Loading,
    Mismatch {
        reason: String,
    },
    NoLyrics {
        reason: String,
    },
    Plain {
        lines: Vec<String>,
    },
    Synced {
        song_id: String,
        duration_seconds: f64,
        lines: Vec<LyricLine>,
        offset_ms: f64,
        position_seconds: f64,
        sampled_at_ms: f64,
        playing: bool,
        rate: f64,
    },
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct LrcLibCandidate {
    #[serde(alias = "track_name")]
    track_name: String,
    #[serde(alias = "artist_name")]
    artist_name: String,
    #[serde(alias = "album_name")]
    album_name: String,
    duration: Option<f64>,
    #[serde(alias = "synced_lyrics")]
    synced_lyrics: Option<String>,
    #[serde(alias = "plain_lyrics")]
    plain_lyrics: Option<String>,
}

#[derive(Clone, Debug)]
struct AmllEntry {
    title: String,
    artists: Vec<String>,
    file: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LookupStatus {
    Idle,
    Loading,
    Ready,
    Miss,
    Mismatch,
    Error,
}

impl Default for LookupStatus {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Default)]
struct ActiveLookup {
    recording_key: String,
    generation: u64,
    status: LookupStatus,
    raw_lyrics: Option<String>,
    error: String,
    misses: u8,
    retry_at: Option<Instant>,
    enriched_started: bool,
}

#[derive(Default)]
struct ProviderRateState {
    last_end: Option<Instant>,
    retry_at: Option<Instant>,
    retry_status: u16,
}

enum FetchFailure {
    RecordingMismatch(String),
    RetryAfter(u64),
    Network,
}

pub struct LyricalState {
    config_dir: Option<PathBuf>,
    cache: Mutex<HashMap<String, String>>,
    amll_index: Mutex<Option<Vec<AmllEntry>>>,
    rate_state: Mutex<ProviderRateState>,
    active: Mutex<ActiveLookup>,
    amll_enrich_lock: Mutex<()>,
}

impl LyricalState {
    pub fn load(config_dir: Option<PathBuf>) -> std::sync::Arc<Self> {
        let mut cache = HashMap::new();
        if let Some(ref dir) = config_dir {
            load_cache_map(dir.join(CACHE_FILE), &mut cache);
        }
        if let Ok(home) = std::env::var("USERPROFILE") {
            let mut legacy = HashMap::new();
            load_cache_map(PathBuf::from(home).join(".lyrical_cache.json"), &mut legacy);
            for (k, v) in legacy {
                let canon = canonical_cache_key(&k);
                cache.entry(canon).or_insert(v);
            }
        }
        std::sync::Arc::new(Self {
            config_dir,
            cache: Mutex::new(cache),
            amll_index: Mutex::new(None),
            rate_state: Mutex::new(ProviderRateState::default()),
            active: Mutex::new(ActiveLookup::default()),
            amll_enrich_lock: Mutex::new(()),
        })
    }

    fn save_cache(&self) {
        let Some(ref dir) = self.config_dir else {
            return;
        };
        if fs::create_dir_all(dir).is_err() {
            return;
        }
        let snapshot = self.cache.lock().unwrap().clone();
        let Ok(bytes) = serde_json::to_vec(&snapshot) else {
            return;
        };
        let path = dir.join(CACHE_FILE);
        let tmp = dir.join(format!("{CACHE_FILE}.tmp"));
        if fs::write(&tmp, bytes).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

fn load_cache_map(path: PathBuf, out: &mut HashMap<String, String>) {
    let Ok(bytes) = fs::read(path) else {
        return;
    };
    let Ok(map) = serde_json::from_slice::<HashMap<String, serde_json::Value>>(&bytes) else {
        return;
    };
    for (k, v) in map {
        if k == "_v" {
            continue;
        }
        if let Some(s) = v.as_str() {
            if !s.trim().is_empty() {
                out.insert(k, s.to_string());
            }
        }
    }
}

fn canonical_cache_key(raw: &str) -> String {
    let Some(rest) = raw.strip_prefix("recording-v3:") else {
        return raw.to_string();
    };
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(rest) {
        if let Ok(compact) = serde_json::to_string(&val) {
            return format!("recording-v3:{compact}");
        }
    }
    raw.to_string()
}

fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = true;
    for ch in text.chars() {
        for lc in ch.to_lowercase() {
            if lc.is_alphanumeric() {
                out.push(lc);
                last_space = false;
            } else if lc.is_whitespace() && !last_space {
                out.push(' ');
                last_space = true;
            }
        }
    }
    out.trim().to_string()
}

pub fn title_key(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = true;
    for ch in text.chars() {
        if ch == '\'' || ch == '’' {
            continue;
        }
        for lc in ch.to_lowercase() {
            if lc.is_alphanumeric() {
                out.push(lc);
                last_space = false;
            } else if !last_space {
                out.push(' ');
                last_space = true;
            }
        }
    }
    out.trim().to_string()
}

pub fn recording_key(title: &str, artist: &str, album: &str, duration: f64) -> String {
    let len = if duration.is_finite() && duration > 0.0 {
        duration.round() as i64
    } else {
        0
    };
    let json = serde_json::to_string(&(normalize(title), normalize(artist), normalize(album), len))
        .unwrap_or_else(|_| "[]".into());
    format!("recording-v3:{json}")
}

fn split_artists(raw: &str) -> Vec<String> {
    let mut normalized = raw.to_lowercase();
    for sep in [
        " featuring ",
        " feat. ",
        " feat ",
        " ft. ",
        " ft ",
        " with ",
        " & ",
    ] {
        normalized = normalized.replace(sep, ",");
    }
    normalized = normalized.replace(';', ",");
    normalized
        .split(',')
        .map(normalize)
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn artist_matches(candidate: &str, expected: &str) -> bool {
    let c_norm = normalize(candidate);
    let e_norm = normalize(expected);
    if c_norm.is_empty() || e_norm.is_empty() {
        return c_norm.is_empty() && e_norm.is_empty();
    }
    if c_norm == e_norm {
        return true;
    }
    let left = split_artists(candidate);
    let right = split_artists(expected);
    left.iter().any(|a| right.contains(a))
}

#[cfg(test)]
pub fn recordings_match(
    expected_title: &str,
    expected_artist: &str,
    expected_duration: f64,
    cand_title: &str,
    cand_artist: &str,
    cand_duration: f64,
) -> bool {
    if title_key(expected_title).is_empty() || title_key(expected_title) != title_key(cand_title) {
        return false;
    }
    if (!expected_artist.trim().is_empty() || !cand_artist.trim().is_empty())
        && !artist_matches(cand_artist, expected_artist)
    {
        return false;
    }
    if expected_duration > 0.0
        && cand_duration > 0.0
        && (expected_duration - cand_duration).abs() > 8.0
    {
        return false;
    }
    true
}

fn parse_timestamp_inner(inner: &str) -> Option<f64> {
    let (min_str, rest) = inner.split_once(':')?;
    let (sec_str, frac_str) = match rest.split_once('.') {
        Some((s, f)) => (s, Some(f)),
        None => (rest, None),
    };
    if min_str.is_empty() || min_str.len() > 3 || sec_str.len() != 2 {
        return None;
    }
    let min: f64 = min_str.parse().ok()?;
    let sec: f64 = sec_str.parse().ok()?;
    if sec >= 60.0 {
        return None;
    }
    let frac_secs = match frac_str {
        Some(f) if (1..=3).contains(&f.len()) => {
            let frac: f64 = f.parse().ok()?;
            frac / 10_f64.powi(f.len() as i32)
        }
        Some(_) => return None,
        None => 0.0,
    };
    Some(min * 60.0 + sec + frac_secs)
}

fn parse_word_tags(raw_text: &str) -> (String, Vec<LyricWord>) {
    let mut words = Vec::new();
    let mut plain = String::new();
    let mut rest = raw_text;
    let mut prefix = String::new();

    while let Some(start) = rest.find('<') {
        let before = &rest[..start];
        let after_lt = &rest[start + 1..];
        if let Some(end) = after_lt.find('>') {
            let tag = &after_lt[..end];
            if let Some(time_seconds) = parse_timestamp_inner(tag) {
                if words.is_empty() {
                    prefix.push_str(before);
                } else if let Some(last) = words.last_mut() {
                    let prev: &mut LyricWord = last;
                    prev.text.push_str(before);
                }
                plain.push_str(before);
                words.push(LyricWord {
                    time_seconds,
                    text: String::new(),
                });
                rest = &after_lt[end + 1..];
                continue;
            }
        }
        plain.push_str(before);
        plain.push('<');
        rest = after_lt;
    }

    plain.push_str(rest);
    if !words.is_empty() {
        if !prefix.is_empty() {
            words[0].text = format!("{prefix}{}", words[0].text);
        }
        if let Some(last) = words.last_mut() {
            last.text.push_str(rest);
        }
    }

    if words.len() < 2 {
        return (raw_text.trim().to_string(), Vec::new());
    }
    (plain.trim().to_string(), words)
}

fn is_music_marker(cleaned: &str) -> bool {
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed
        .chars()
        .all(|c| c.is_whitespace() || matches!(c, '♪' | '♫' | '♬'))
    {
        return true;
    }
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        return true;
    }
    let stripped = trimmed
        .trim_start_matches('(')
        .trim_end_matches(')')
        .to_lowercase();
    matches!(
        stripped.as_str(),
        "instrumental"
            | "music"
            | "guitar"
            | "solo"
            | "beat"
            | "break"
            | "drop"
            | "intro"
            | "outro"
    )
}

pub fn parse_lrc(raw: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for line in raw.lines() {
        let mut cursor = line.trim();
        let mut timestamps = Vec::new();
        while cursor.starts_with('[') {
            if let Some(close) = cursor.find(']') {
                let inner = &cursor[1..close];
                if let Some(ts) = parse_timestamp_inner(inner) {
                    timestamps.push(ts);
                    cursor = cursor[close + 1..].trim_start();
                    continue;
                }
            }
            break;
        }
        if timestamps.is_empty() {
            continue;
        }
        let (cleaned, words) = parse_word_tags(cursor);
        if is_music_marker(&cleaned) {
            continue;
        }
        for ts in timestamps {
            lines.push(LyricLine {
                time_seconds: ts,
                text: cleaned.clone(),
                words: words.clone(),
            });
        }
    }
    lines.sort_by(|a, b| a.time_seconds.total_cmp(&b.time_seconds));
    lines
}

fn has_word_tags(raw: &str) -> bool {
    let mut count = 0;
    let mut rest = raw;
    while let Some(s) = rest.find('<') {
        let after = &rest[s + 1..];
        if let Some(e) = after.find('>') {
            if parse_timestamp_inner(&after[..e]).is_some() {
                count += 1;
                if count >= 2 {
                    return true;
                }
            }
            rest = &after[e + 1..];
        } else {
            break;
        }
    }
    false
}

fn duration_fallback(
    cand: &LrcLibCandidate,
    artist: &str,
    album: &str,
    duration: f64,
    difference: Option<f64>,
) -> bool {
    let Some(diff) = difference else {
        return false;
    };
    let tol = (8.0_f64).min((3.0_f64).max(duration * 0.04));
    if diff > tol {
        return false;
    }
    if normalize(&cand.artist_name) != normalize(artist) {
        return false;
    }
    if album.trim().is_empty() || title_key(&cand.album_name) != title_key(album) {
        return false;
    }
    let Some(synced) = cand.synced_lyrics.as_deref() else {
        return false;
    };
    let lines = parse_lrc(synced);
    let mut unique_ms = HashSet::new();
    let mut max_anchor = 0.0_f64;
    for l in &lines {
        unique_ms.insert((l.time_seconds * 1000.0).round() as i64);
        if l.time_seconds > max_anchor {
            max_anchor = l.time_seconds;
        }
    }
    unique_ms.len() >= 3 && max_anchor <= duration + 2.0
}

fn best_lrclib_result<'a>(
    candidates: &'a [LrcLibCandidate],
    title: &str,
    artist: &str,
    album: &str,
    duration: f64,
) -> Option<&'a LrcLibCandidate> {
    let expected_title = title_key(title);
    if expected_title.is_empty() {
        return None;
    }
    let expected_artist = normalize(artist);
    let expected_album = normalize(album);
    let mut best: Option<((bool, bool, bool, bool, bool, i64), &'a LrcLibCandidate)> = None;

    for cand in candidates {
        let has_synced = cand
            .synced_lyrics
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty());
        let has_plain = cand
            .plain_lyrics
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty());
        if !has_synced && !has_plain {
            continue;
        }
        if title_key(&cand.track_name) != expected_title {
            continue;
        }
        if (!expected_artist.is_empty() || !cand.artist_name.trim().is_empty())
            && !artist_matches(&cand.artist_name, artist)
        {
            continue;
        }
        let length = cand.duration.unwrap_or(0.0);
        if !length.is_finite() {
            continue;
        }
        let difference = if duration > 0.0 && length > 0.0 {
            Some((length - duration).abs())
        } else {
            None
        };
        let strict = difference.is_none_or(|d| d <= 2.0);
        if !strict && !duration_fallback(cand, artist, album, duration, difference) {
            continue;
        }
        let score = (
            strict,
            has_synced,
            !expected_artist.is_empty() && normalize(&cand.artist_name) == expected_artist,
            !expected_album.is_empty() && normalize(&cand.album_name) == expected_album,
            difference.is_some(),
            -((difference.unwrap_or(0.0) * 1000.0).round() as i64),
        );
        if best.as_ref().is_none_or(|(b_score, _)| score > *b_score) {
            best = Some((score, cand));
        }
    }
    best.map(|(_, c)| c)
}

fn compatible_word_lyrics(enhanced: &str, baseline: &str) -> bool {
    let collect = |raw: &str| -> HashMap<String, Vec<f64>> {
        let mut map: HashMap<String, Vec<f64>> = HashMap::new();
        for line in parse_lrc(raw) {
            let key = normalize(&line.text);
            if !key.is_empty() {
                map.entry(key).or_default().push(line.time_seconds);
            }
        }
        map
    };
    let source = collect(enhanced);
    let target = collect(baseline);
    if source.is_empty() || target.is_empty() {
        return false;
    }
    let min_len = source.len().min(target.len());
    let shared: Vec<&String> = source.keys().filter(|k| target.contains_key(*k)).collect();
    if shared.len() < 3 || (shared.len() as f64) < (min_len as f64) * 0.6 {
        return false;
    }
    for k in &shared {
        let s_times = &source[*k];
        let t_times = &target[*k];
        let s_ok = s_times
            .iter()
            .all(|a| t_times.iter().any(|b| (a - b).abs() <= 2.0));
        let t_ok = t_times
            .iter()
            .all(|a| s_times.iter().any(|b| (a - b).abs() <= 2.0));
        if !s_ok || !t_ok {
            return false;
        }
    }
    true
}

pub fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3 / 2);
    for &b in input.as_bytes() {
        if matches!(
            b,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~'
        ) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn parse_ttml_time(raw: &str) -> Option<f64> {
    let val = raw.trim();
    if val.is_empty() {
        return None;
    }
    if !val.contains(':') {
        let num = val.strip_suffix('s').unwrap_or(val);
        return num.parse::<f64>().ok().filter(|n| n.is_finite());
    }
    let mut total = 0.0_f64;
    for part in val.split(':') {
        let n: f64 = part.parse().ok()?;
        if !n.is_finite() {
            return None;
        }
        total = total * 60.0 + n;
    }
    Some(total)
}

fn fmt_lrc_time(seconds: f64) -> String {
    let clamped = seconds.max(0.0);
    let mins = (clamped / 60.0).floor() as u64;
    let secs = clamped - (mins as f64) * 60.0;
    format!("{mins:02}:{secs:06.3}")
}

fn decode_xml_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        if let Some(semi) = after.find(';') {
            let ent = &after[..semi];
            let decoded = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                    u32::from_str_radix(&ent[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                }
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            if let Some(ch) = decoded {
                out.push(ch);
                rest = &after[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn collapse_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_ws = false;
    for ch in input.chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(ch);
            in_ws = false;
        }
    }
    out
}

fn extract_xml_attr(attrs: &str, target_local: &str) -> Option<String> {
    let bytes = attrs.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'='
            && bytes[i] != b'>'
            && bytes[i] != b'/'
        {
            i += 1;
        }
        if name_start == i {
            i += 1;
            continue;
        }
        let name = &attrs[name_start..i];
        let local = name.rsplit(':').next().unwrap_or(name);
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let quote = bytes[i];
        if quote == b'"' || quote == b'\'' {
            i += 1;
            let val_start = i;
            while i < bytes.len() && bytes[i] != quote {
                i += 1;
            }
            let val = &attrs[val_start..i];
            if i < bytes.len() {
                i += 1;
            }
            if local.eq_ignore_ascii_case(target_local) {
                return Some(decode_xml_entities(val));
            }
        }
    }
    None
}

fn xml_local_tag(raw_tag: &str) -> &str {
    let trimmed = raw_tag.trim_start_matches('/').trim();
    let end = trimmed
        .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
        .unwrap_or(trimmed.len());
    let full = &trimmed[..end];
    full.rsplit(':').next().unwrap_or(full)
}

fn strip_tags_text(fragment: &str) -> String {
    let mut out = String::new();
    let mut rest = fragment;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        if let Some(gt) = rest[lt + 1..].find('>') {
            rest = &rest[lt + 1 + gt + 1..];
        } else {
            break;
        }
    }
    out.push_str(rest);
    decode_xml_entities(&out)
}

fn find_matching_close_tag(body: &str, open_end: usize, tag_name: &str) -> Option<(usize, usize)> {
    let mut depth = 1usize;
    let mut pos = open_end;
    while let Some(rel_lt) = body[pos..].find('<') {
        let lt = pos + rel_lt;
        let Some(rel_gt) = body[lt + 1..].find('>') else {
            break;
        };
        let gt = lt + 1 + rel_gt;
        let inside = body[lt + 1..gt].trim();
        let is_closing = inside.starts_with('/');
        let is_self = inside.ends_with('/');
        if xml_local_tag(inside).eq_ignore_ascii_case(tag_name) {
            if is_closing {
                depth -= 1;
                if depth == 0 {
                    return Some((lt, gt + 1));
                }
            } else if !is_self {
                depth += 1;
            }
        }
        pos = gt + 1;
    }
    None
}

pub fn ttml_to_enhanced_lrc(xml: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut word_synced_lines = 0usize;
    let mut cursor = 0usize;

    while let Some(rel_lt) = xml[cursor..].find('<') {
        let lt = cursor + rel_lt;
        let Some(rel_gt) = xml[lt + 1..].find('>') else {
            break;
        };
        let gt = lt + 1 + rel_gt;
        let tag_inner = xml[lt + 1..gt].trim();
        if tag_inner.starts_with('?')
            || tag_inner.starts_with('!')
            || tag_inner.starts_with('/')
            || xml_local_tag(tag_inner) != "p"
        {
            cursor = gt + 1;
            continue;
        }
        let self_closing = tag_inner.ends_with('/');
        let attrs = tag_inner
            .trim_start_matches(|c: char| !c.is_ascii_whitespace())
            .trim_end_matches('/');
        let begin = extract_xml_attr(attrs, "begin").and_then(|s| parse_ttml_time(&s));
        let p_end = extract_xml_attr(attrs, "end").and_then(|s| parse_ttml_time(&s));

        if self_closing {
            cursor = gt + 1;
            continue;
        }
        let Some((close_start, close_after)) = find_matching_close_tag(xml, gt + 1, "p") else {
            break;
        };
        cursor = close_after;
        let Some(line_begin) = begin else {
            continue;
        };
        let p_body = &xml[gt + 1..close_start];

        let mut words: Vec<(f64, Option<f64>, String)> = Vec::new();
        let mut fallback_plain = String::new();
        let mut inner_pos = 0usize;
        while let Some(s_rel) = p_body[inner_pos..].find('<') {
            let s_lt = inner_pos + s_rel;
            let Some(s_rel_gt) = p_body[s_lt + 1..].find('>') else {
                break;
            };
            let s_gt = s_lt + 1 + s_rel_gt;
            let s_inner = p_body[s_lt + 1..s_gt].trim();
            if s_inner.starts_with('/') || xml_local_tag(s_inner) != "span" {
                inner_pos = s_gt + 1;
                continue;
            }
            let span_attrs = s_inner
                .trim_start_matches(|c: char| !c.is_ascii_whitespace())
                .trim_end_matches('/');
            let has_role = extract_xml_attr(span_attrs, "role").is_some_and(|r| !r.is_empty());

            let (span_text_raw, after_span) = if s_inner.ends_with('/') {
                (String::new(), s_gt + 1)
            } else if let Some((c_start, c_after)) =
                find_matching_close_tag(p_body, s_gt + 1, "span")
            {
                (strip_tags_text(&p_body[s_gt + 1..c_start]), c_after)
            } else {
                inner_pos = s_gt + 1;
                continue;
            };

            let tail_end = p_body[after_span..]
                .find('<')
                .map_or(p_body.len(), |n| after_span + n);
            let tail_raw = decode_xml_entities(&p_body[after_span..tail_end]);
            inner_pos = after_span;

            if has_role {
                continue;
            }
            let combined = collapse_whitespace(&format!("{span_text_raw}{tail_raw}"));
            fallback_plain.push_str(&combined);
            if combined.trim().is_empty() {
                continue;
            }
            if let Some(wb) =
                extract_xml_attr(span_attrs, "begin").and_then(|s| parse_ttml_time(&s))
            {
                let we = extract_xml_attr(span_attrs, "end").and_then(|s| parse_ttml_time(&s));
                words.push((wb, we, combined));
            }
        }

        if !words.is_empty() {
            let mut row = format!("[{}]", fmt_lrc_time(line_begin));
            for (wb, _, text) in &words {
                row.push_str(&format!("<{}>{text}", fmt_lrc_time(*wb)));
            }
            if let Some(tail) = p_end.or_else(|| words.last().and_then(|w| w.1)) {
                row.push_str(&format!("<{}>", fmt_lrc_time(tail)));
            }
            lines.push(row);
            word_synced_lines += 1;
        } else {
            let plain = if fallback_plain.trim().is_empty() {
                collapse_whitespace(&strip_tags_text(p_body))
            } else {
                fallback_plain
            };
            let trimmed = plain.trim();
            if !trimmed.is_empty() {
                lines.push(format!("[{}]{trimmed}", fmt_lrc_time(line_begin)));
            }
        }
    }

    if word_synced_lines < 4 {
        return None;
    }
    Some(lines.join("\n"))
}

fn parse_amll_index_bytes(data: &[u8]) -> Vec<AmllEntry> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RawIndexObj {
        metadata: Option<Vec<(String, Vec<String>)>>,
        raw_lyric_file: Option<String>,
    }
    let mut entries = Vec::new();
    let text = String::from_utf8_lossy(data);
    for line in text.lines() {
        let Ok(obj) = serde_json::from_str::<RawIndexObj>(line) else {
            continue;
        };
        let Some(fname) = obj.raw_lyric_file else {
            continue;
        };
        if !is_safe_amll_filename(&fname) {
            continue;
        }
        let mut title = None;
        let mut artists = Vec::new();
        if let Some(pairs) = obj.metadata {
            for (k, vals) in pairs {
                if k == "musicName" {
                    title = vals.into_iter().next();
                } else if k == "artists" {
                    artists = vals;
                }
            }
        }
        if let Some(t) = title.filter(|s| !s.trim().is_empty()) {
            entries.push(AmllEntry {
                title: t,
                artists,
                file: fname,
            });
        }
    }
    entries
}

fn is_safe_amll_filename(name: &str) -> bool {
    name.ends_with(".ttml")
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[cfg(windows)]
pub(crate) fn winhttp_get_bytes(
    host: &str,
    path_and_query: &str,
    accept_header: &str,
    max_bytes: usize,
) -> Result<(u16, Option<u64>, Vec<u8>), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Networking::WinHttp::{
        WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
        WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
        WinHttpSendRequest, WinHttpSetTimeouts, INTERNET_DEFAULT_HTTPS_PORT,
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_QUERY_RETRY_AFTER, WINHTTP_QUERY_STATUS_CODE,
    };

    struct Handle(*mut core::ffi::c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }

    fn to_wide_nul(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe {
        let agent_w = to_wide_nul("Peek/0.1.0 (https://github.com/MellowLaw/peek)");
        let h_session = Handle(WinHttpOpen(
            PCWSTR(agent_w.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if h_session.0.is_null() {
            return Err("WinHttpOpen failed".into());
        }
        let _ = WinHttpSetTimeouts(h_session.0, 4000, 4000, 6000, 8000);

        let host_w = to_wide_nul(host);
        let h_connect = Handle(WinHttpConnect(
            h_session.0,
            PCWSTR(host_w.as_ptr()),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ));
        if h_connect.0.is_null() {
            return Err("WinHttpConnect failed".into());
        }

        let verb_w = to_wide_nul("GET");
        let path_w = to_wide_nul(path_and_query);
        let h_request = Handle(WinHttpOpenRequest(
            h_connect.0,
            PCWSTR(verb_w.as_ptr()),
            PCWSTR(path_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if h_request.0.is_null() {
            return Err("WinHttpOpenRequest failed".into());
        }

        let headers_w: Vec<u16> = accept_header.encode_utf16().collect();
        WinHttpSendRequest(h_request.0, Some(&headers_w), None, 0, 0, 0)
            .map_err(|e| format!("WinHttpSendRequest failed: {e}"))?;
        WinHttpReceiveResponse(h_request.0, std::ptr::null_mut())
            .map_err(|e| format!("WinHttpReceiveResponse failed: {e}"))?;

        let mut status: u32 = 0;
        let mut status_len = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            h_request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut core::ffi::c_void),
            &mut status_len,
            std::ptr::null_mut(),
        )
        .map_err(|e| format!("WinHttpQueryHeaders failed: {e}"))?;

        let mut retry_after = None;
        if status == 429 || status == 503 {
            let mut ra: u32 = 0;
            let mut ra_len = std::mem::size_of::<u32>() as u32;
            if WinHttpQueryHeaders(
                h_request.0,
                WINHTTP_QUERY_RETRY_AFTER | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some(&mut ra as *mut u32 as *mut core::ffi::c_void),
                &mut ra_len,
                std::ptr::null_mut(),
            )
            .is_ok()
                && ra > 0
            {
                retry_after = Some(ra as u64);
            }
        }

        let mut body = Vec::new();
        loop {
            let mut avail: u32 = 0;
            WinHttpQueryDataAvailable(h_request.0, &mut avail)
                .map_err(|e| format!("WinHttpQueryDataAvailable failed: {e}"))?;
            if avail == 0 {
                break;
            }
            let to_read = (avail as usize).min(65536);
            if body.len() + to_read > max_bytes {
                return Err("HTTPS response exceeded size cap".into());
            }
            let mut chunk = vec![0u8; to_read];
            let mut read: u32 = 0;
            WinHttpReadData(
                h_request.0,
                chunk.as_mut_ptr() as *mut core::ffi::c_void,
                to_read as u32,
                &mut read,
            )
            .map_err(|e| format!("WinHttpReadData failed: {e}"))?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read as usize]);
        }
        Ok((status as u16, retry_after, body))
    }
}

#[cfg(not(windows))]
pub(crate) fn winhttp_get_bytes(
    _host: &str,
    _path_and_query: &str,
    _accept_header: &str,
    _max_bytes: usize,
) -> Result<(u16, Option<u64>, Vec<u8>), String> {
    Err("Windows WinHttp required".into())
}

fn winhttp_get(
    host: &str,
    path_and_query: &str,
    max_bytes: usize,
) -> Result<(u16, Option<u64>, Vec<u8>), String> {
    winhttp_get_bytes(
        host,
        path_and_query,
        "Accept: application/json\r\n",
        max_bytes,
    )
}

fn rate_limited_lrclib_get(
    state: &LyricalState,
    path_and_query: &str,
) -> Result<(u16, Vec<u8>), FetchFailure> {
    let sleep_dur = {
        let guard = state.rate_state.lock().unwrap();
        if let Some(retry_at) = guard.retry_at {
            let now = Instant::now();
            if retry_at > now {
                let _ = guard.retry_status;
                return Err(FetchFailure::RetryAfter((retry_at - now).as_secs().max(1)));
            }
        }
        guard.last_end.and_then(|last| {
            let elapsed = last.elapsed();
            let gap = Duration::from_millis(250);
            if elapsed < gap {
                Some(gap - elapsed)
            } else {
                None
            }
        })
    };
    if let Some(dur) = sleep_dur {
        std::thread::sleep(dur);
    }

    let res = winhttp_get("lrclib.net", path_and_query, LRCLIB_MAX_BYTES);
    let mut guard = state.rate_state.lock().unwrap();
    guard.last_end = Some(Instant::now());
    let (status, retry_after, body) = res.map_err(|_| FetchFailure::Network)?;
    if status == 429 || status == 503 {
        let secs = retry_after
            .unwrap_or(if status == 429 { 60 } else { 15 })
            .max(1);
        guard.retry_at = Some(Instant::now() + Duration::from_secs(secs));
        guard.retry_status = status;
        return Err(FetchFailure::RetryAfter(secs));
    }
    Ok((status, body))
}

fn fetch_lrclib_lyrics(
    state: &LyricalState,
    title: &str,
    artist: &str,
    album: &str,
    duration: f64,
) -> Result<Option<String>, FetchFailure> {
    let mut candidates = Vec::new();
    let mut plain_fallback: Option<String> = None;

    let mut query = format!(
        "/api/get?track_name={}&artist_name={}",
        url_encode(title.trim()),
        url_encode(artist.trim())
    );
    if !album.trim().is_empty() {
        query.push_str("&album_name=");
        query.push_str(&url_encode(album.trim()));
    }
    if (1.0..=3600.0).contains(&duration) {
        query.push_str(&format!("&duration={}", duration.round() as u64));
    }

    let (st1, body1) = rate_limited_lrclib_get(state, &query)?;
    if st1 >= 500 {
        return Err(FetchFailure::Network);
    }
    if st1 == 200 {
        if let Ok(item) = serde_json::from_slice::<LrcLibCandidate>(&body1) {
            candidates.push(item);
            if let Some(best) = best_lrclib_result(&candidates, title, artist, album, duration) {
                if let Some(synced) = best
                    .synced_lyrics
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                {
                    return Ok(Some(synced.to_string()));
                }
                plain_fallback = best
                    .plain_lyrics
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_string);
            }
        }
    }

    let exact_record = candidates.first().cloned();
    let search_q = format!(
        "/api/search?track_name={}&artist_name={}",
        url_encode(title.trim()),
        url_encode(artist.trim())
    );
    if let Ok((st2, body2)) = rate_limited_lrclib_get(state, &search_q) {
        if st2 == 200 {
            if let Ok(mut list) = serde_json::from_slice::<Vec<LrcLibCandidate>>(&body2) {
                candidates.extend(list.clone());
                if let Some(ref ex) = exact_record {
                    list.insert(0, ex.clone());
                }
                if let Some(best) = best_lrclib_result(&list, title, artist, album, duration) {
                    if let Some(lrc) = best
                        .synced_lyrics
                        .as_deref()
                        .filter(|s| !s.trim().is_empty())
                        .or_else(|| {
                            best.plain_lyrics
                                .as_deref()
                                .filter(|s| !s.trim().is_empty())
                        })
                    {
                        return Ok(Some(lrc.to_string()));
                    }
                }
            }
        }
    }

    let freetext = format!("{} {}", title_key(title), artist.trim());
    let free_q = format!("/api/search?q={}", url_encode(freetext.trim()));
    if let Ok((st3, body3)) = rate_limited_lrclib_get(state, &free_q) {
        if st3 == 200 {
            if let Ok(mut list) = serde_json::from_slice::<Vec<LrcLibCandidate>>(&body3) {
                candidates.extend(list.clone());
                if let Some(ref ex) = exact_record {
                    list.insert(0, ex.clone());
                }
                if let Some(best) = best_lrclib_result(&list, title, artist, album, duration) {
                    if let Some(lrc) = best
                        .synced_lyrics
                        .as_deref()
                        .filter(|s| !s.trim().is_empty())
                        .or_else(|| {
                            best.plain_lyrics
                                .as_deref()
                                .filter(|s| !s.trim().is_empty())
                        })
                    {
                        return Ok(Some(lrc.to_string()));
                    }
                }
            }
        }
    }

    if let Some(plain) = plain_fallback {
        return Ok(Some(plain));
    }
    if best_lrclib_result(&candidates, title, artist, album, 0.0).is_some() {
        return Err(FetchFailure::RecordingMismatch(
            "Lyrics length differs from playing track".into(),
        ));
    }
    Ok(None)
}

fn is_file_fresh(path: &PathBuf) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .or_else(|_| {
            UNIX_EPOCH
                .elapsed()
                .map(|_| Duration::from_secs(AMLL_MAX_AGE_SECS + 1))
        })
        .is_ok_and(|age| age.as_secs() < AMLL_MAX_AGE_SECS)
}

fn find_amll_file(state: &LyricalState, title: &str, artist: &str) -> Option<String> {
    {
        let cached = state.amll_index.lock().unwrap();
        if let Some(ref entries) = *cached {
            return pick_amll_match(entries, title, artist);
        }
    }

    let peek_idx_path = state.config_dir.as_ref().map(|d| d.join(AMLL_INDEX_FILE));
    let mut data: Option<Vec<u8>> = None;

    if let Some(ref p) = peek_idx_path {
        if is_file_fresh(p) {
            data = fs::read(p).ok();
        }
    }
    if data.is_none() {
        if let Ok(home) = std::env::var("USERPROFILE") {
            let legacy_idx = PathBuf::from(home).join(".lyrical_amll_index.jsonl");
            if is_file_fresh(&legacy_idx) {
                data = fs::read(legacy_idx).ok();
            }
        }
    }
    if data.is_none() {
        if let Ok((200, _, downloaded)) = winhttp_get(
            "raw.githubusercontent.com",
            "/amll-dev/amll-ttml-db/main/metadata/raw-lyrics-index.jsonl",
            AMLL_INDEX_MAX_BYTES,
        ) {
            if let Some(ref p) = peek_idx_path {
                if let Some(parent) = p.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::write(p, &downloaded);
            }
            data = Some(downloaded);
        } else if let Some(ref p) = peek_idx_path {
            data = fs::read(p).ok();
        }
    }

    let bytes = data?;
    let entries = parse_amll_index_bytes(&bytes);
    let picked = pick_amll_match(&entries, title, artist);
    *state.amll_index.lock().unwrap() = Some(entries);
    picked
}

fn pick_amll_match(entries: &[AmllEntry], title: &str, artist: &str) -> Option<String> {
    let norm_title = normalize(title);
    let norm_artist = normalize(artist);
    if norm_title.is_empty() {
        return None;
    }
    let mut best: Option<&str> = None;
    for e in entries {
        if normalize(&e.title) != norm_title {
            continue;
        }
        if !norm_artist.is_empty() && !e.artists.is_empty() {
            let matched = e.artists.iter().any(|a| {
                let na = normalize(a);
                !na.is_empty() && (na.contains(&norm_artist) || norm_artist.contains(&na))
            });
            if !matched {
                continue;
            }
        }
        if best.is_none_or(|b| e.file.as_str() > b) {
            best = Some(e.file.as_str());
        }
    }
    best.map(str::to_string)
}

fn spawn_word_enrichment(
    state: std::sync::Arc<LyricalState>,
    key: String,
    generation: u64,
    title: String,
    artist: String,
    baseline: String,
) {
    tokio::task::spawn_blocking(move || {
        let _guard = state.amll_enrich_lock.lock().unwrap();
        if state.active.lock().unwrap().generation != generation {
            return;
        }
        let Some(fname) = find_amll_file(&state, &title, &artist) else {
            return;
        };
        let path = format!("/amll-dev/amll-ttml-db/main/raw-lyrics/{fname}");
        let Ok((200, _, ttml_bytes)) =
            winhttp_get("raw.githubusercontent.com", &path, AMLL_TTML_MAX_BYTES)
        else {
            return;
        };
        let xml = String::from_utf8_lossy(&ttml_bytes);
        let Some(enhanced) = ttml_to_enhanced_lrc(&xml) else {
            return;
        };
        if !compatible_word_lyrics(&enhanced, &baseline) {
            return;
        }
        let mut should_save = false;
        {
            let mut active = state.active.lock().unwrap();
            if active.generation == generation && active.recording_key == key {
                active.raw_lyrics = Some(enhanced.clone());
                active.status = LookupStatus::Ready;
                should_save = true;
            }
        }
        if should_save {
            state.cache.lock().unwrap().insert(key, enhanced);
            state.save_cache();
        }
    });
}

fn snapshot_from_raw(raw: &str, song_id: String, duration_seconds: f64) -> LyricalSnapshot {
    let synced = parse_lrc(raw);
    if !synced.is_empty() {
        return LyricalSnapshot::Synced {
            song_id,
            duration_seconds: duration_seconds.max(0.0),
            lines: synced,
            offset_ms: 0.0,
            position_seconds: 0.0,
            sampled_at_ms: 0.0,
            playing: false,
            rate: 1.0,
        };
    }
    let plain: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    if plain.is_empty() {
        LyricalSnapshot::NoLyrics {
            reason: "No lyrics available".into(),
        }
    } else {
        LyricalSnapshot::Plain { lines: plain }
    }
}

#[tauri::command]
pub async fn get_lyrical_snapshot(
    prefs: State<'_, PreferencesState>,
    lyrical_state: State<'_, std::sync::Arc<LyricalState>>,
    title: String,
    artist: String,
    album: String,
    duration_seconds: f64,
) -> Result<LyricalSnapshot, String> {
    if !prefs.0.lock().unwrap().lyrics_enabled {
        return Ok(LyricalSnapshot::Disabled);
    }
    if title.trim().is_empty() {
        return Ok(LyricalSnapshot::NoLyrics {
            reason: "Waiting for song details…".into(),
        });
    }

    let state = lyrical_state.inner().clone();
    let key = recording_key(&title, &artist, &album, duration_seconds);

    let cached_hit = state.cache.lock().unwrap().get(&key).cloned();
    if let Some(cached_raw) = cached_hit {
        let mut need_enrich = false;
        let gen_val = {
            let mut active = state.active.lock().unwrap();
            if active.recording_key != key {
                active.generation += 1;
                active.recording_key = key.clone();
                active.status = LookupStatus::Ready;
                active.raw_lyrics = Some(cached_raw.clone());
                active.error.clear();
                active.misses = 0;
                active.retry_at = None;
                active.enriched_started = false;
            }
            if !active.enriched_started
                && !has_word_tags(&cached_raw)
                && !parse_lrc(&cached_raw).is_empty()
            {
                active.enriched_started = true;
                need_enrich = true;
            }
            active.generation
        };
        if need_enrich {
            spawn_word_enrichment(
                state,
                key.clone(),
                gen_val,
                title,
                artist,
                cached_raw.clone(),
            );
        }
        return Ok(snapshot_from_raw(&cached_raw, key, duration_seconds));
    }

    enum Action {
        Return(LyricalSnapshot),
        Start(u64),
    }

    let action = {
        let mut active = state.active.lock().unwrap();
        let now = Instant::now();
        if active.recording_key != key {
            active.generation += 1;
            active.recording_key = key.clone();
            active.status = LookupStatus::Loading;
            active.raw_lyrics = None;
            active.error.clear();
            active.misses = 0;
            active.retry_at = None;
            active.enriched_started = false;
            Action::Start(active.generation)
        } else {
            match active.status {
                LookupStatus::Loading => Action::Return(LyricalSnapshot::Loading),
                LookupStatus::Ready => {
                    if let Some(ref raw) = active.raw_lyrics {
                        Action::Return(snapshot_from_raw(raw, key.clone(), duration_seconds))
                    } else {
                        active.status = LookupStatus::Loading;
                        Action::Start(active.generation)
                    }
                }
                LookupStatus::Miss => {
                    if active.retry_at.is_some_and(|t| now >= t) && active.misses < 3 {
                        active.status = LookupStatus::Loading;
                        active.retry_at = None;
                        Action::Start(active.generation)
                    } else {
                        Action::Return(LyricalSnapshot::NoLyrics {
                            reason: if active.error.is_empty() {
                                "No lyrics found for this recording".into()
                            } else {
                                active.error.clone()
                            },
                        })
                    }
                }
                LookupStatus::Mismatch => Action::Return(LyricalSnapshot::Mismatch {
                    reason: active.error.clone(),
                }),
                LookupStatus::Error => {
                    if active.retry_at.is_some_and(|t| now >= t) {
                        active.status = LookupStatus::Loading;
                        active.retry_at = None;
                        Action::Start(active.generation)
                    } else {
                        Action::Return(LyricalSnapshot::Offline)
                    }
                }
                LookupStatus::Idle => {
                    active.status = LookupStatus::Loading;
                    Action::Start(active.generation)
                }
            }
        }
    };

    let generation = match action {
        Action::Return(snap) => return Ok(snap),
        Action::Start(g) => g,
    };

    let bg_state = state.clone();
    let bg_key = key.clone();
    let bg_title = title.clone();
    let bg_artist = artist.clone();
    tokio::task::spawn_blocking(move || {
        let fetched =
            fetch_lrclib_lyrics(&bg_state, &bg_title, &bg_artist, &album, duration_seconds);
        let mut enrich_payload: Option<String> = None;
        let mut save_cache = false;

        {
            let mut active = bg_state.active.lock().unwrap();
            if active.generation != generation || active.recording_key != bg_key {
                return;
            }
            match fetched {
                Ok(Some(raw)) => {
                    active.status = LookupStatus::Ready;
                    active.raw_lyrics = Some(raw.clone());
                    active.error.clear();
                    active.misses = 0;
                    active.retry_at = None;
                    if !has_word_tags(&raw) && !parse_lrc(&raw).is_empty() {
                        active.enriched_started = true;
                        enrich_payload = Some(raw.clone());
                    }
                    bg_state.cache.lock().unwrap().insert(bg_key.clone(), raw);
                    save_cache = true;
                }
                Ok(None) => {
                    active.misses = active.misses.saturating_add(1);
                    active.status = LookupStatus::Miss;
                    active.raw_lyrics = None;
                    if active.misses < 3 {
                        let wait = if active.misses == 1 { 5 } else { 20 };
                        active.retry_at = Some(Instant::now() + Duration::from_secs(wait));
                        active.error = "No matching lyrics found yet. Retrying…".into();
                    } else {
                        active.retry_at = None;
                        active.error = "No lyrics found for this recording".into();
                    }
                }
                Err(FetchFailure::RecordingMismatch(msg)) => {
                    active.misses = 3;
                    active.status = LookupStatus::Mismatch;
                    active.error = msg;
                    active.retry_at = None;
                }
                Err(FetchFailure::RetryAfter(secs)) => {
                    active.status = LookupStatus::Error;
                    active.retry_at = Some(Instant::now() + Duration::from_secs(secs));
                    active.error = "Lyrics provider busy; retrying shortly…".into();
                }
                Err(FetchFailure::Network) => {
                    active.status = LookupStatus::Error;
                    active.retry_at = Some(Instant::now() + Duration::from_secs(15));
                    active.error = "Lyrics lookup offline; retrying shortly…".into();
                }
            }
        }

        if save_cache {
            bg_state.save_cache();
        }
        if let Some(baseline) = enrich_payload {
            spawn_word_enrichment(bg_state, bg_key, generation, bg_title, bg_artist, baseline);
        }
    });

    Ok(LyricalSnapshot::Loading)
}

fn find_lyrical_executable() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("Lyrical.exe"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(
            cwd.join("..")
                .join("lyrics")
                .join("dist")
                .join("Lyrical.exe"),
        );
        candidates.push(
            cwd.join("..")
                .join("..")
                .join("lyrics")
                .join("dist")
                .join("Lyrical.exe"),
        );
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        candidates.push(
            PathBuf::from(&home)
                .join("Documents")
                .join("PROJECTS")
                .join("lyrics")
                .join("dist")
                .join("Lyrical.exe"),
        );
    }
    if let Ok(local_app) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local_app).join("Lyrical").join("Lyrical.exe"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

#[tauri::command]
pub fn open_in_lyrical() -> Result<(), String> {
    #[cfg(windows)]
    if crate::windows::focus_window_by_title("Lyrical") {
        return Ok(());
    }
    if let Some(exe) = find_lyrical_executable() {
        std::process::Command::new(&exe)
            .spawn()
            .map_err(|e| format!("Could not start Lyrical: {e}"))?;
        return Ok(());
    }
    Err("Lyrical window not found and Lyrical.exe could not be located".into())
}

#[cfg(test)]
fn decode_chunked(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < body.len() {
        let rest = &body[pos..];
        let Some(crlf) = rest.windows(2).position(|w| w == b"\r\n") else {
            return Err("Malformed chunked response".into());
        };
        let size_line = std::str::from_utf8(&rest[..crlf]).map_err(|e| e.to_string())?;
        let hex = size_line.split(';').next().unwrap_or("").trim();
        let len = usize::from_str_radix(hex, 16).map_err(|_| "Invalid chunk length".to_string())?;
        pos += crlf + 2;
        if len == 0 {
            return Ok(out);
        }
        if pos + len + 2 > body.len() || out.len() + len > LRCLIB_MAX_BYTES {
            return Err("Chunked body exceeds limit".into());
        }
        out.extend_from_slice(&body[pos..pos + len]);
        pos += len + 2;
    }
    Ok(out)
}

#[cfg(test)]
fn parse_http_response(raw: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| "Incomplete HTTP headers".to_string())?;
    let head = std::str::from_utf8(&raw[..sep]).map_err(|e| e.to_string())?;
    let mut lines = head.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| "Missing status line".to_string())?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "Invalid HTTP status".to_string())?;
    let mut is_chunked = false;
    for header in lines {
        if let Some((name, val)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("transfer-encoding")
                && val.to_ascii_lowercase().contains("chunked")
            {
                is_chunked = true;
            }
        }
    }
    let body_slice = &raw[sep + 4..];
    let body = if is_chunked {
        decode_chunked(body_slice)?
    } else {
        body_slice.to_vec()
    };
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_and_word_enhanced_lrc() {
        let lrc = "\
[00:05.00](Instrumental)
[00:12.50]<00:12.50>Hello <00:13.00>world
[00:20.123]Second line
[00:25][01:05.00]Repeated chorus";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].text, "Hello world");
        assert!((lines[0].time_seconds - 12.5).abs() < 1e-6);
        assert_eq!(lines[0].words.len(), 2);
        assert_eq!(lines[0].words[0].text, "Hello ");
        assert_eq!(lines[1].text, "Second line");
        assert!((lines[1].time_seconds - 20.123).abs() < 1e-6);
        assert_eq!(lines[2].text, "Repeated chorus");
        assert!((lines[2].time_seconds - 25.0).abs() < 1e-6);
        assert_eq!(lines[3].text, "Repeated chorus");
        assert!((lines[3].time_seconds - 65.0).abs() < 1e-6);
    }

    #[test]
    fn enforces_recording_identity_and_duration_tolerance() {
        assert!(recordings_match(
            "Starlight (Remastered)",
            "Muse feat. Guest",
            240.0,
            "Starlight (Remastered)",
            "Muse",
            243.0
        ));
        assert!(!recordings_match(
            "Starlight",
            "Muse",
            240.0,
            "Starlight",
            "Muse",
            265.0
        ));
        assert!(!recordings_match(
            "Starlight",
            "Muse",
            240.0,
            "Hysteria",
            "Muse",
            240.0
        ));
    }

    #[test]
    fn decodes_chunked_http_response() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n7\r\n{\"ok\":t\r\n4\r\nrue}\r\n0\r\n\r\n";
        let (status, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, br#"{"ok":true}"#);
    }

    #[test]
    fn scores_lrclib_candidates_and_rejects_wrong_duration() {
        let wrong_dur = LrcLibCandidate {
            track_name: "Song".into(),
            artist_name: "Artist".into(),
            album_name: "Other".into(),
            duration: Some(300.0),
            synced_lyrics: Some("[00:10.00]Line".into()),
            plain_lyrics: None,
        };
        let plain_exact = LrcLibCandidate {
            track_name: "Song".into(),
            artist_name: "Artist".into(),
            album_name: "Album".into(),
            duration: Some(200.0),
            synced_lyrics: None,
            plain_lyrics: Some("Line".into()),
        };
        let synced_album = LrcLibCandidate {
            track_name: "Song".into(),
            artist_name: "Artist".into(),
            album_name: "Album".into(),
            duration: Some(201.0),
            synced_lyrics: Some("[00:10.00]Line".into()),
            plain_lyrics: None,
        };
        let list = vec![wrong_dur.clone(), plain_exact, synced_album];
        let best = best_lrclib_result(&list, "Song", "Artist", "Album", 200.0).unwrap();
        assert_eq!(best.duration, Some(201.0));
        assert!(best.synced_lyrics.is_some());

        let wrong_only = [wrong_dur];
        let rejected = best_lrclib_result(&wrong_only, "Song", "Artist", "Album", 200.0);
        assert!(rejected.is_none());
    }

    #[test]
    fn converts_amll_ttml_preserving_word_spaces_and_skipping_roles() {
        let ttml = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata"><body><div>
          <p begin="00:10.000" end="00:12.000"><span begin="00:10.000" end="00:10.500">Sa</span> <span begin="00:10.600" end="00:11.800">mga</span><span ttm:role="x-translation">In the</span></p>
          <p begin="00:13.000" end="00:15.000"><span begin="00:13.000" end="00:13.600">gabi</span> <span begin="00:13.700" end="00:14.900">na</span></p>
          <p begin="00:16.000" end="00:18.000"><span begin="00:16.000" end="00:16.500">ako</span> <span begin="00:16.600" end="00:17.800">lang</span></p>
          <p begin="00:19.000" end="00:21.000"><span begin="00:19.000" end="00:19.500">mag</span> <span begin="00:19.600" end="00:20.900">isa</span></p>
        </div></body></tt>"#;
        let lrc = ttml_to_enhanced_lrc(ttml).expect("4 word-synced lines should convert");
        let parsed = parse_lrc(&lrc);
        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed[0].text, "Sa mga");
        assert_eq!(parsed[0].words.len(), 3);
        assert_eq!(parsed[0].words[0].text, "Sa ");
        assert_eq!(parsed[0].words[1].text, "mga");
        assert_eq!(parsed[0].words[2].text, "");
        assert!((parsed[0].words[2].time_seconds - 12.0).abs() < 1e-6);
        assert!(!lrc.contains("In the"));
    }

    #[test]
    fn validates_compatible_word_lyrics_alignment() {
        let base = "\
[00:10.00]First line here
[00:15.00]Second line here
[00:20.00]Third line here";
        let good = "\
[00:10.50]<00:10.50>First <00:11.00>line here
[00:14.20]<00:14.20>Second <00:15.00>line here
[00:20.90]<00:20.90>Third <00:21.20>line here";
        let shifted = "\
[00:15.00]<00:15.00>First line here
[00:20.00]<00:20.00>Second line here
[00:25.00]<00:25.00>Third line here";
        assert!(compatible_word_lyrics(good, base));
        assert!(!compatible_word_lyrics(shifted, base));
    }

    #[test]
    fn percent_encodes_query_values() {
        assert_eq!(
            url_encode("Hello World & You!"),
            "Hello%20World%20%26%20You%21"
        );
    }
}
