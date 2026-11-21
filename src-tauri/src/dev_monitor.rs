use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::watch;

use crate::activity::{ActivityManager, Priority};

const TAIL_BYTES: u64 = 1024 * 1024;
const MAX_FILES_PER_PROVIDER: usize = 64;
const FIVE_HOURS_MS: u64 = 5 * 3_600 * 1000;
const SEVEN_DAYS_MS: u64 = 7 * 86_400 * 1000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentStatus {
    Working,
    NeedsApproval,
    Completed,
    Idle,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitWindow {
    pub label: String,
    pub used_percent: f64,
    pub window_minutes: Option<u64>,
    pub resets_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub status: AgentStatus,
    pub active_model: String,
    pub sessions_count: usize,
    pub today_input_tokens: u64,
    pub today_output_tokens: u64,
    pub today_cached_tokens: u64,
    pub today_total_tokens: u64,
    pub today_cost_usd: Option<f64>,
    pub five_hour_tokens: u64,
    pub weekly_tokens: u64,
    pub five_hour_resets_at: Option<u64>,
    pub weekly_resets_at: Option<u64>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cached_tokens: u64,
    pub total_tokens: u64,
    pub total_cost_usd: Option<f64>,
    pub plan_type: String,
    pub quota_note: String,
    pub rate_limits: Vec<RateLimitWindow>,
    pub last_active_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevMonitorSnapshot {
    pub providers: Vec<ProviderUsage>,
    pub claude_statusline_connected: bool,
}

pub struct DevMonitorState {
    enabled: watch::Sender<bool>,
    last_statuses: Mutex<HashMap<String, AgentStatus>>,
}

pub fn start(app: AppHandle) -> DevMonitorState {
    let (enabled, mut rx) = watch::channel(false);
    tauri::async_runtime::spawn(async move {
        let mut seeded = false;
        let config_dir = app.path().app_config_dir().ok();
        while rx.wait_for(|on| *on).await.is_ok() {
            loop {
                if !*rx.borrow() {
                    break;
                }
                let cfg = config_dir.clone();
                let snap = tokio::task::spawn_blocking(move || read_all_usage(cfg.as_deref()))
                    .await
                    .unwrap_or_else(|_| DevMonitorSnapshot {
                        providers: Vec::new(),
                        claude_statusline_connected: false,
                    });

                {
                    let state = app.state::<DevMonitorState>();
                    let mut prev = state.last_statuses.lock().unwrap();
                    if seeded {
                        for p in &snap.providers {
                            if let Some(old) = prev.get(&p.id) {
                                if *old != p.status {
                                    match p.status {
                                        AgentStatus::NeedsApproval => {
                                            app.state::<ActivityManager>().push_activity(
                                                &p.name,
                                                format!("{} needs approval", p.name),
                                                p.active_model.clone(),
                                                Priority::High,
                                                5000,
                                                Some("ai"),
                                                false,
                                                true,
                                            );
                                        }
                                        AgentStatus::Completed => {
                                            app.state::<ActivityManager>().push_activity(
                                                &p.name,
                                                format!("{} finished", p.name),
                                                p.active_model.clone(),
                                                Priority::Normal,
                                                4000,
                                                Some("ai"),
                                                false,
                                                true,
                                            );
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    prev.clear();
                    for p in &snap.providers {
                        prev.insert(p.id.clone(), p.status.clone());
                    }
                    seeded = true;
                }

                let _ = app.emit("dev-monitor-update", &snap);

                tokio::select! {
                    _ = rx.changed() => {}
                    _ = tokio::time::sleep(Duration::from_secs(4)) => {}
                }
            }
        }
    });
    DevMonitorState {
        enabled,
        last_statuses: Mutex::new(HashMap::new()),
    }
}

fn read_tail_lines(path: &Path) -> Vec<String> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let Ok(meta) = file.metadata() else {
        return Vec::new();
    };
    let len = meta.len();
    let skip_first = len > TAIL_BYTES;
    if skip_first && file.seek(SeekFrom::End(-(TAIL_BYTES as i64))).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.lines();
    if skip_first {
        let _ = lines.next();
    }
    lines.map(str::to_string).collect()
}

fn collect_recent_jsonl(root: &Path, limit: usize) -> Vec<(PathBuf, u64)> {
    let mut stack = vec![root.to_path_buf()];
    let mut found = Vec::new();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                found.push((p, mtime));
            }
        }
    }
    found.sort_by(|a, b| b.1.cmp(&a.1));
    found.truncate(limit);
    found
}

fn collect_antigravity_transcripts(brain_root: &Path, limit: usize) -> Vec<(PathBuf, u64)> {
    let Ok(entries) = fs::read_dir(brain_root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let transcript = dir
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl");
        if let Ok(meta) = fs::metadata(&transcript) {
            if meta.is_file() {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                found.push((transcript, mtime));
            }
        }
    }
    found.sort_by(|a, b| b.1.cmp(&a.1));
    found.truncate(limit);
    found
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Converts a civil Gregorian date to days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u64, d: u64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m_adj = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * m_adj + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + (doe as i64) - 719_468
}

/// Parse an ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SS(.fff)?Z`) into Unix epoch ms.
pub fn parse_iso_utc_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let m: u64 = s.get(5..7)?.parse().ok()?;
    let d: u64 = s.get(8..10)?.parse().ok()?;
    let hh: u64 = s.get(11..13)?.parse().ok()?;
    let mm: u64 = s.get(14..16)?.parse().ok()?;
    let ss: u64 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    let days = days_from_civil(y, m, d);
    if days < 0 {
        return None;
    }
    let secs = (days as u64) * 86_400 + hh * 3_600 + mm * 60 + ss;
    Some(secs * 1000)
}

#[cfg(windows)]
fn local_day_window_ms() -> (u64, u64) {
    use windows::Win32::Foundation::SYSTEMTIME;
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};

    let (local, utc) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let to_ms = |st: SYSTEMTIME| -> u64 {
        let days = days_from_civil(st.wYear as i64, st.wMonth as u64, st.wDay as u64).max(0) as u64;
        let secs = days * 86_400
            + (st.wHour as u64) * 3_600
            + (st.wMinute as u64) * 60
            + (st.wSecond as u64);
        secs * 1000
    };

    let local_midnight_days =
        days_from_civil(local.wYear as i64, local.wMonth as u64, local.wDay as u64).max(0) as u64;
    let local_midnight_as_utc_ms = local_midnight_days * 86_400 * 1000;
    let local_ms = to_ms(local) as i64;
    let utc_ms = to_ms(utc) as i64;
    let offset_ms = local_ms - utc_ms;
    let start = (local_midnight_as_utc_ms as i64 - offset_ms).max(0) as u64;
    (start, start + 86_400_000)
}

#[cfg(not(windows))]
fn local_day_window_ms() -> (u64, u64) {
    let now = now_epoch_ms();
    let start = (now / 86_400_000) * 86_400_000;
    (start, start + 86_400_000)
}

#[cfg(windows)]
fn is_pid_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    const STILL_ACTIVE: u32 = 259;
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code).is_ok() && code == STILL_ACTIVE;
        let _ = CloseHandle(handle);
        ok
    }
}

#[cfg(not(windows))]
fn is_pid_alive(_pid: u32) -> bool {
    false
}

fn estimate_claude_cost(
    model: &str,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
) -> Option<f64> {
    let lower = model.to_lowercase();
    let (in_p, out_p, cr_p, cw_p) = if lower.contains("opus") {
        (15.0, 75.0, 1.5, 18.75)
    } else if lower.contains("sonnet") {
        (3.0, 15.0, 0.3, 3.75)
    } else if lower.contains("haiku") {
        (0.8, 4.0, 0.08, 1.0)
    } else {
        return None;
    };
    let cost = (input as f64 * in_p
        + output as f64 * out_p
        + cache_read as f64 * cr_p
        + cache_write as f64 * cw_p)
        / 1_000_000.0;
    Some((cost * 10000.0).round() / 10000.0)
}

#[derive(Default)]
struct ClaudeParsed {
    status: Option<AgentStatus>,
    active_model: String,
    last_active_ms: u64,
    // (timestamp_ms, model, input, output, cache_read, cache_write)
    messages: HashMap<String, (u64, String, u64, u64, u64, u64)>,
}

fn parse_claude_jsonl_lines(
    lines: &[String],
    mtime_ms: u64,
    live_sessions: &HashSet<String>,
    now_ms: u64,
) -> Option<ClaudeParsed> {
    let mut session_id = String::new();
    let mut model = String::new();
    let mut last_stop = String::new();
    let mut last_type = String::new();
    let mut messages: HashMap<String, (u64, String, u64, u64, u64, u64)> = HashMap::new();
    let mut seq = 0usize;

    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(sid) = v.get("sessionId").and_then(Value::as_str) {
            if !sid.is_empty() {
                session_id = sid.to_string();
            }
        }
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        if !kind.is_empty() {
            last_type = kind.to_string();
        }
        let line_ts = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_iso_utc_ms)
            .unwrap_or(mtime_ms);
        if kind == "assistant" {
            if let Some(msg) = v.get("message") {
                let mut msg_model = model.clone();
                if let Some(m) = msg.get("model").and_then(Value::as_str) {
                    if m != "<synthetic>" && !m.is_empty() {
                        model = m.to_string();
                        msg_model = model.clone();
                    }
                }
                if let Some(sr) = msg.get("stop_reason").and_then(Value::as_str) {
                    last_stop = sr.to_string();
                }
                if let Some(u) = msg.get("usage") {
                    let msg_key = msg
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            seq += 1;
                            format!("anon-{seq}")
                        });
                    let inp = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                    let out = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                    let cr = u
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let cw = u
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    messages.insert(msg_key, (line_ts, msg_model, inp, out, cr, cw));
                }
            }
        }
    }

    if messages.is_empty() {
        return None;
    }

    let age_ms = now_ms.saturating_sub(mtime_ms);
    let is_live = !session_id.is_empty() && live_sessions.contains(&session_id);
    let status = if last_stop == "end_turn" && age_ms > 4_000 {
        AgentStatus::Completed
    } else if last_stop == "tool_use" && (is_live || age_ms < 300_000) && age_ms > 12_000 {
        AgentStatus::NeedsApproval
    } else if is_live || (age_ms < 20_000 && last_type != "assistant") {
        AgentStatus::Working
    } else if last_stop == "end_turn" {
        AgentStatus::Completed
    } else {
        AgentStatus::Idle
    };

    Some(ClaudeParsed {
        status: Some(status),
        active_model: model,
        last_active_ms: mtime_ms,
        messages,
    })
}

#[derive(Default)]
struct AntigravityParsed {
    active_model: String,
    last_active_ms: u64,
    entries: Vec<(u64, u64, u64, u64)>, // (ts_ms, inp, cached, out)
}

fn parse_antigravity_transcript_lines<'a, I>(lines: I, mtime_ms: u64) -> Option<AntigravityParsed>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut parsed = AntigravityParsed {
        last_active_ms: mtime_ms,
        ..Default::default()
    };

    for line in lines {
        if !line.contains("PLANNER_RESPONSE") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("PLANNER_RESPONSE") {
            continue;
        }
        if let Some(m) = v.get("model").and_then(Value::as_str) {
            if !m.trim().is_empty() {
                parsed.active_model = m.to_string();
            }
        }
        let ts = v
            .get("created_at")
            .and_then(Value::as_str)
            .and_then(parse_iso_utc_ms)
            .unwrap_or(mtime_ms);
        let inp = v.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
        let cached = v
            .get("cache_read_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let out = v.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
        if inp > 0 || cached > 0 || out > 0 {
            parsed.entries.push((ts, inp, cached, out));
        }
    }

    if parsed.entries.is_empty() && parsed.active_model.is_empty() {
        return None;
    }
    Some(parsed)
}

#[derive(Default)]
struct CodexParsed {
    status: Option<AgentStatus>,
    active_model: String,
    last_active_ms: u64,
    total_input: u64,
    total_cached: u64,
    total_output: u64,
    total_tokens: u64,
    today_input: u64,
    today_cached: u64,
    today_output: u64,
    today_tokens: u64,
    token_events: Vec<(u64, u64)>,
    plan_type: String,
    rate_limits: Vec<RateLimitWindow>,
}

fn rate_limit_label(fallback: &str, window_minutes: Option<u64>) -> String {
    match window_minutes {
        Some(m) if m <= 360 => "5-hour limit".into(),
        Some(m) if m <= 11_520 => "Weekly limit".into(),
        Some(m) => format!("Plan limit ({}d)", ((m + 720) / 1440).max(1)),
        None => fallback.into(),
    }
}

fn accumulate_window_tokens<I>(events: I, now_ms: u64) -> (u64, u64, Option<u64>, Option<u64>)
where
    I: IntoIterator<Item = (u64, u64)>,
{
    let h5_start = now_ms.saturating_sub(FIVE_HOURS_MS);
    let d7_start = now_ms.saturating_sub(SEVEN_DAYS_MS);
    let future_cap = now_ms.saturating_add(60_000);
    let mut tok_5h = 0u64;
    let mut tok_7d = 0u64;
    let mut min_5h: Option<u64> = None;
    let mut min_7d: Option<u64> = None;

    for (ts, tokens) in events {
        if tokens == 0 || ts > future_cap {
            continue;
        }
        if ts >= d7_start {
            tok_7d = tok_7d.saturating_add(tokens);
            min_7d = Some(min_7d.map_or(ts, |m| m.min(ts)));
        }
        if ts >= h5_start {
            tok_5h = tok_5h.saturating_add(tokens);
            min_5h = Some(min_5h.map_or(ts, |m| m.min(ts)));
        }
    }

    (
        tok_5h,
        tok_7d,
        min_5h.map(|m| m.saturating_add(FIVE_HOURS_MS) / 1000),
        min_7d.map(|m| m.saturating_add(SEVEN_DAYS_MS) / 1000),
    )
}

fn parse_codex_jsonl_lines(
    lines: &[String],
    mtime_ms: u64,
    day_start_ms: u64,
    day_end_ms: u64,
    now_ms: u64,
) -> Option<CodexParsed> {
    let mut model = String::new();
    let mut total_input = 0u64;
    let mut total_cached = 0u64;
    let mut total_output = 0u64;
    let mut total_tokens = 0u64;
    let mut today_input = 0u64;
    let mut today_cached = 0u64;
    let mut today_output = 0u64;
    let mut today_tokens = 0u64;
    let mut token_events: Vec<(u64, u64)> = Vec::new();
    let mut saw_last_usage_today = false;
    let mut plan_type = String::new();
    let mut rate_limits = Vec::new();
    let mut last_event = String::new();

    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let top = v.get("type").and_then(Value::as_str).unwrap_or("");
        let line_ts = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_iso_utc_ms)
            .unwrap_or(mtime_ms);
        let payload = v.get("payload");
        if top == "turn_context" {
            if let Some(m) = payload.and_then(|p| p.get("model")).and_then(Value::as_str) {
                model = m.to_string();
            }
        } else if top == "event_msg" {
            if let Some(p) = payload {
                let ptype = p.get("type").and_then(Value::as_str).unwrap_or("");
                if !ptype.is_empty() && ptype != "token_count" {
                    last_event = ptype.to_string();
                }
                if ptype == "token_count" {
                    if let Some(u) = p.pointer("/info/total_token_usage") {
                        total_input = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                        total_cached = u
                            .get("cached_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        total_output = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                        total_tokens = u.get("total_tokens").and_then(Value::as_u64).unwrap_or(0);
                    }
                    if let Some(lu) = p.pointer("/info/last_token_usage") {
                        let ev_total = lu.get("total_tokens").and_then(Value::as_u64).unwrap_or(0);
                        if ev_total > 0 {
                            token_events.push((line_ts, ev_total));
                        }
                        if line_ts >= day_start_ms && line_ts < day_end_ms {
                            saw_last_usage_today = true;
                            today_input +=
                                lu.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                            today_cached += lu
                                .get("cached_input_tokens")
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            today_output +=
                                lu.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                            today_tokens += ev_total;
                        }
                    }
                    if let Some(rl) = p.get("rate_limits") {
                        if let Some(pt) = rl.get("plan_type").and_then(Value::as_str) {
                            plan_type = pt.to_string();
                        }
                        let mut next_limits = Vec::new();
                        for (field, fallback) in
                            [("primary", "Primary"), ("secondary", "Secondary")]
                        {
                            if let Some(win) = rl.get(field) {
                                if let Some(pct) = win.get("used_percent").and_then(Value::as_f64) {
                                    let win_mins =
                                        win.get("window_minutes").and_then(Value::as_u64);
                                    next_limits.push(RateLimitWindow {
                                        label: rate_limit_label(fallback, win_mins),
                                        used_percent: pct,
                                        window_minutes: win_mins,
                                        resets_at: win.get("resets_at").and_then(Value::as_u64),
                                    });
                                }
                            }
                        }
                        if !next_limits.is_empty() {
                            rate_limits = next_limits;
                        }
                    }
                }
            }
        }
    }

    if token_events.is_empty() && total_tokens > 0 {
        token_events.push((mtime_ms, total_tokens));
    }

    if !saw_last_usage_today && mtime_ms >= day_start_ms && mtime_ms < day_end_ms {
        today_input = total_input;
        today_cached = total_cached;
        today_output = total_output;
        today_tokens = total_tokens;
    }

    if total_tokens == 0 && rate_limits.is_empty() && model.is_empty() {
        return None;
    }

    let age_ms = now_ms.saturating_sub(mtime_ms);
    let status = match last_event.as_str() {
        "exec_approval_request" | "apply_patch_approval_request" => AgentStatus::NeedsApproval,
        "task_complete" | "turn_aborted" => AgentStatus::Completed,
        _ if age_ms < 20_000 => AgentStatus::Working,
        _ => AgentStatus::Idle,
    };

    Some(CodexParsed {
        status: Some(status),
        active_model: model,
        last_active_ms: mtime_ms,
        total_input,
        total_cached,
        total_output,
        total_tokens,
        today_input,
        today_cached,
        today_output,
        today_tokens,
        token_events,
        plan_type,
        rate_limits,
    })
}

fn status_priority(s: &AgentStatus) -> u8 {
    match s {
        AgentStatus::NeedsApproval => 4,
        AgentStatus::Working => 3,
        AgentStatus::Completed => 2,
        AgentStatus::Idle => 1,
    }
}

fn pick_best_status(cur: AgentStatus, cand: AgentStatus) -> AgentStatus {
    if status_priority(&cand) > status_priority(&cur) {
        cand
    } else {
        cur
    }
}

fn parse_claude_quota_cache(raw: &str, now_secs: u64) -> Option<(Vec<RateLimitWindow>, String)> {
    let trimmed = raw.trim().trim_start_matches('\u{feff}');
    let v: Value = serde_json::from_str(trimmed).ok()?;
    let model = v
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    let extract = |obj: &Value, label: &str, mins: u64| -> Option<RateLimitWindow> {
        let pct_val = obj
            .get("usedPercentage")
            .or_else(|| obj.get("used_percentage"))
            .and_then(Value::as_f64)?;
        let resets_at = obj
            .get("resetsAt")
            .or_else(|| obj.get("resets_at"))
            .and_then(Value::as_u64);
        let used_percent = if resets_at.is_some_and(|ts| ts <= now_secs) {
            0.0
        } else {
            pct_val.clamp(0.0, 100.0)
        };
        Some(RateLimitWindow {
            label: label.to_string(),
            used_percent,
            window_minutes: Some(mins),
            resets_at,
        })
    };

    let mut windows = Vec::new();
    if let Some(five) = v.get("fiveHour").filter(|w| !w.is_null()) {
        if let Some(win) = extract(five, "5-hour limit", 300) {
            windows.push(win);
        }
    }
    if let Some(seven) = v.get("sevenDay").filter(|w| !w.is_null()) {
        if let Some(win) = extract(seven, "Weekly limit", 10_080) {
            windows.push(win);
        }
    }
    if windows.is_empty() {
        return None;
    }
    Some((windows, model))
}

fn is_claude_statusline_connected(home: &Path) -> bool {
    let settings_path = home.join(".claude").join("settings.json");
    let Ok(raw) = fs::read_to_string(settings_path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    v.pointer("/statusLine/command")
        .and_then(Value::as_str)
        .is_some_and(|cmd| cmd.contains("peek-claude-statusline.ps1"))
}

fn read_all_usage(config_dir: Option<&Path>) -> DevMonitorSnapshot {
    let home = std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default();
    let now_ms = now_epoch_ms();
    let now_secs = now_ms / 1000;
    let (day_start_ms, day_end_ms) = local_day_window_ms();

    // 1. Claude Code (~/.claude)
    let claude_root = home.join(".claude");
    let claude_available = claude_root.is_dir();
    let mut live_claude_ids = HashSet::new();
    if let Ok(entries) = fs::read_dir(claude_root.join("sessions")) {
        for entry in entries.flatten() {
            if let Ok(bytes) = fs::read(entry.path()) {
                if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                    let pid = v.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
                    let sid = v.get("sessionId").and_then(Value::as_str).unwrap_or("");
                    if pid > 0 && !sid.is_empty() && is_pid_alive(pid) {
                        live_claude_ids.insert(sid.to_string());
                    }
                }
            }
        }
    }

    let mut claude_status = AgentStatus::Idle;
    let mut claude_model = String::new();
    let mut claude_sessions = 0usize;
    let mut claude_last_ms = 0u64;
    let mut claude_today_in = 0u64;
    let mut claude_today_out = 0u64;
    let mut claude_today_cached = 0u64;
    let mut claude_today_cost = 0.0f64;
    let mut claude_has_today_cost = false;
    let mut claude_tot_in = 0u64;
    let mut claude_tot_out = 0u64;
    let mut claude_tot_cached = 0u64;
    let mut claude_tot_cost = 0.0f64;
    let mut claude_has_tot_cost = false;
    let mut claude_events: Vec<(u64, u64)> = Vec::new();

    if claude_available {
        for (path, mtime) in
            collect_recent_jsonl(&claude_root.join("projects"), MAX_FILES_PER_PROVIDER)
        {
            let lines = read_tail_lines(&path);
            if let Some(p) = parse_claude_jsonl_lines(&lines, mtime, &live_claude_ids, now_ms) {
                claude_sessions += 1;
                claude_last_ms = claude_last_ms.max(p.last_active_ms);
                if claude_model.is_empty() && !p.active_model.is_empty() {
                    claude_model = p.active_model;
                }
                if let Some(st) = p.status {
                    claude_status = pick_best_status(claude_status, st);
                }
                for (ts, mdl, inp, out, cr, cw) in p.messages.into_values() {
                    let cached = cr + cw;
                    claude_events.push((ts, inp + out + cached));
                    claude_tot_in += inp;
                    claude_tot_out += out;
                    claude_tot_cached += cached;
                    if let Some(c) = estimate_claude_cost(&mdl, inp, out, cr, cw) {
                        claude_tot_cost += c;
                        claude_has_tot_cost = true;
                    }
                    if ts >= day_start_ms && ts < day_end_ms {
                        claude_today_in += inp;
                        claude_today_out += out;
                        claude_today_cached += cached;
                        if let Some(c) = estimate_claude_cost(&mdl, inp, out, cr, cw) {
                            claude_today_cost += c;
                            claude_has_today_cost = true;
                        }
                    }
                }
            }
        }
    }

    let (c_5h, c_7d, c_5h_reset, c_7d_reset) = accumulate_window_tokens(claude_events, now_ms);
    let claude_cache = config_dir
        .and_then(|dir| fs::read_to_string(dir.join("claude_quota.json")).ok())
        .and_then(|raw| parse_claude_quota_cache(&raw, now_secs));
    let (claude_live_limits, claude_plan_type, claude_quota_note) = match claude_cache {
        Some((windows, cache_model)) => {
            if claude_model.is_empty() && !cache_model.is_empty() {
                claude_model = cache_model;
            }
            (
                windows,
                "Claude live".into(),
                "Live limits from Claude Code statusLine".into(),
            )
        }
        None => (
            Vec::new(),
            String::new(),
            "Local 5h / weekly budget vs Claude Code session logs".into(),
        ),
    };
    let claude_usage = ProviderUsage {
        id: "claude".into(),
        name: "Claude".into(),
        available: claude_available,
        status: claude_status,
        active_model: if claude_model.is_empty() {
            "Claude Code".into()
        } else {
            claude_model
        },
        sessions_count: claude_sessions,
        today_input_tokens: claude_today_in,
        today_output_tokens: claude_today_out,
        today_cached_tokens: claude_today_cached,
        today_total_tokens: claude_today_in + claude_today_out + claude_today_cached,
        today_cost_usd: claude_has_today_cost
            .then(|| (claude_today_cost * 1000.0).round() / 1000.0),
        five_hour_tokens: c_5h,
        weekly_tokens: c_7d,
        five_hour_resets_at: c_5h_reset,
        weekly_resets_at: c_7d_reset,
        total_input_tokens: claude_tot_in,
        total_output_tokens: claude_tot_out,
        total_cached_tokens: claude_tot_cached,
        total_tokens: claude_tot_in + claude_tot_out + claude_tot_cached,
        total_cost_usd: claude_has_tot_cost.then(|| (claude_tot_cost * 1000.0).round() / 1000.0),
        plan_type: claude_plan_type,
        quota_note: claude_quota_note,
        rate_limits: claude_live_limits,
        last_active_ms: claude_last_ms,
    };

    // 2. Gemini / Antigravity (~/.gemini/antigravity/brain/*/.system_generated/logs/transcript.jsonl)
    let gemini_brain = home.join(".gemini").join("antigravity").join("brain");
    let gemini_available = gemini_brain.is_dir();
    let mut gem_status = AgentStatus::Idle;
    let mut gem_model = String::new();
    let mut gem_sessions = 0usize;
    let mut gem_last_ms = 0u64;
    let mut gem_today_in = 0u64;
    let mut gem_today_out = 0u64;
    let mut gem_today_cached = 0u64;
    let mut gem_tot_in = 0u64;
    let mut gem_tot_out = 0u64;
    let mut gem_tot_cached = 0u64;
    let mut gem_events: Vec<(u64, u64)> = Vec::new();

    if gemini_available {
        for (path, mtime) in collect_antigravity_transcripts(&gemini_brain, MAX_FILES_PER_PROVIDER)
        {
            let Ok(file) = File::open(&path) else {
                continue;
            };
            let mut raw_lines = Vec::new();
            let reader = BufReader::new(file);
            for line in reader.lines().map_while(Result::ok) {
                if line.contains("PLANNER_RESPONSE") {
                    raw_lines.push(line);
                }
            }
            let refs: Vec<&str> = raw_lines.iter().map(String::as_str).collect();
            if let Some(p) = parse_antigravity_transcript_lines(refs, mtime) {
                gem_sessions += 1;
                gem_last_ms = gem_last_ms.max(p.last_active_ms);
                if gem_model.is_empty() && !p.active_model.is_empty() {
                    gem_model = p.active_model;
                }
                if now_ms.saturating_sub(mtime) < 25_000 {
                    gem_status = pick_best_status(gem_status, AgentStatus::Working);
                }
                for (ts, inp, cached, out) in p.entries {
                    gem_events.push((ts, inp + cached + out));
                    gem_tot_in += inp;
                    gem_tot_cached += cached;
                    gem_tot_out += out;
                    if ts >= day_start_ms && ts < day_end_ms {
                        gem_today_in += inp;
                        gem_today_cached += cached;
                        gem_today_out += out;
                    }
                }
            }
        }
    }

    let (g_5h, g_7d, g_5h_reset, g_7d_reset) = accumulate_window_tokens(gem_events, now_ms);
    let gemini_usage = ProviderUsage {
        id: "gemini".into(),
        name: "Gemini / Antigravity".into(),
        available: gemini_available,
        status: gem_status,
        active_model: if gem_model.is_empty() {
            "Antigravity".into()
        } else {
            gem_model
        },
        sessions_count: gem_sessions,
        today_input_tokens: gem_today_in,
        today_output_tokens: gem_today_out,
        today_cached_tokens: gem_today_cached,
        today_total_tokens: gem_today_in + gem_today_out + gem_today_cached,
        today_cost_usd: None,
        five_hour_tokens: g_5h,
        weekly_tokens: g_7d,
        five_hour_resets_at: g_5h_reset,
        weekly_resets_at: g_7d_reset,
        total_input_tokens: gem_tot_in,
        total_output_tokens: gem_tot_out,
        total_cached_tokens: gem_tot_cached,
        total_tokens: gem_tot_in + gem_tot_out + gem_tot_cached,
        total_cost_usd: None,
        plan_type: String::new(),
        quota_note: "Local 5h / weekly budget vs Antigravity transcripts".into(),
        rate_limits: Vec::new(),
        last_active_ms: gem_last_ms,
    };

    // 3. Codex (~/.codex)
    let codex_root = home.join(".codex");
    let codex_available = codex_root.is_dir();
    let mut cx_status = AgentStatus::Idle;
    let mut cx_model = String::new();
    let mut cx_sessions = 0usize;
    let mut cx_last_ms = 0u64;
    let mut cx_today_in = 0u64;
    let mut cx_today_out = 0u64;
    let mut cx_today_cached = 0u64;
    let mut cx_today_total = 0u64;
    let mut cx_tot_in = 0u64;
    let mut cx_tot_out = 0u64;
    let mut cx_tot_cached = 0u64;
    let mut cx_tot_total = 0u64;
    let mut cx_events: Vec<(u64, u64)> = Vec::new();
    let mut cx_plan = String::new();
    let mut cx_limits = Vec::new();

    if codex_available {
        for (path, mtime) in
            collect_recent_jsonl(&codex_root.join("sessions"), MAX_FILES_PER_PROVIDER)
        {
            let lines = read_tail_lines(&path);
            if let Some(p) =
                parse_codex_jsonl_lines(&lines, mtime, day_start_ms, day_end_ms, now_ms)
            {
                cx_sessions += 1;
                cx_last_ms = cx_last_ms.max(p.last_active_ms);
                if cx_model.is_empty() && !p.active_model.is_empty() {
                    cx_model = p.active_model;
                }
                if cx_plan.is_empty() && !p.plan_type.is_empty() {
                    cx_plan = p.plan_type;
                }
                if cx_limits.is_empty() && !p.rate_limits.is_empty() {
                    cx_limits = p.rate_limits;
                }
                if let Some(st) = p.status {
                    cx_status = pick_best_status(cx_status, st);
                }
                cx_today_in += p.today_input;
                cx_today_cached += p.today_cached;
                cx_today_out += p.today_output;
                cx_today_total += p.today_tokens;
                cx_tot_in += p.total_input;
                cx_tot_cached += p.total_cached;
                cx_tot_out += p.total_output;
                cx_tot_total += p.total_tokens;
                cx_events.extend(p.token_events);
            }
        }
    }

    let (x_5h, x_7d, x_5h_reset, x_7d_reset) = accumulate_window_tokens(cx_events, now_ms);
    let codex_note = if cx_limits.is_empty() {
        "Local 5h / weekly budget vs Codex session logs".into()
    } else if cx_plan.is_empty() {
        "Plan rate limits reported in Codex logs".into()
    } else {
        format!("Plan: {cx_plan}")
    };

    let codex_usage = ProviderUsage {
        id: "codex".into(),
        name: "Codex".into(),
        available: codex_available,
        status: cx_status,
        active_model: if cx_model.is_empty() {
            "Codex".into()
        } else {
            cx_model
        },
        sessions_count: cx_sessions,
        today_input_tokens: cx_today_in,
        today_output_tokens: cx_today_out,
        today_cached_tokens: cx_today_cached,
        today_total_tokens: cx_today_total,
        today_cost_usd: None,
        five_hour_tokens: x_5h,
        weekly_tokens: x_7d,
        five_hour_resets_at: x_5h_reset,
        weekly_resets_at: x_7d_reset,
        total_input_tokens: cx_tot_in,
        total_output_tokens: cx_tot_out,
        total_cached_tokens: cx_tot_cached,
        total_tokens: cx_tot_total,
        total_cost_usd: None,
        plan_type: cx_plan,
        quota_note: codex_note,
        rate_limits: cx_limits,
        last_active_ms: cx_last_ms,
    };

    let claude_statusline_connected = is_claude_statusline_connected(&home);
    DevMonitorSnapshot {
        providers: vec![claude_usage, gemini_usage, codex_usage],
        claude_statusline_connected,
    }
}

#[tauri::command]
pub async fn get_dev_monitor(app: AppHandle) -> Result<DevMonitorSnapshot, String> {
    let config_dir = app.path().app_config_dir().ok();
    tokio::task::spawn_blocking(move || read_all_usage(config_dir.as_deref()))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn watch_dev_monitor(state: State<'_, DevMonitorState>, enabled: bool) {
    state.enabled.send_replace(enabled);
}

#[tauri::command]
pub async fn install_claude_statusline(app: AppHandle) -> Result<DevMonitorSnapshot, String> {
    let config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let home = std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .map_err(|_| "USERPROFILE not found".to_string())?;
    let claude_dir = home.join(".claude");
    if !claude_dir.is_dir() {
        return Err("Claude Code (~/.claude) directory not found".into());
    }

    tokio::task::spawn_blocking(move || -> Result<DevMonitorSnapshot, String> {
        fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
        let script_path = config_dir.join("peek-claude-statusline.ps1");
        let cache_path = config_dir.join("claude_quota.json");
        let prev_path = config_dir.join("prev_claude_statusline.json");

        let esc_ps = |p: &Path| p.to_string_lossy().replace('\'', "''");
        let script = format!(
            r#"param()
$raw = [Console]::In.ReadToEnd()
$cachePath = '{cache}'
$prevPath = '{prev}'
$data = $null
if ($raw) {{
  try {{
    $data = $raw | ConvertFrom-Json -ErrorAction Stop
    $out = @{{
      updatedAt = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
      model = if ($data.model -and $data.model.display_name) {{ [string]$data.model.display_name }} else {{ "" }}
      fiveHour = if ($data.rate_limits -and $data.rate_limits.five_hour) {{
        @{{
          usedPercentage = $data.rate_limits.five_hour.used_percentage
          resetsAt = $data.rate_limits.five_hour.resets_at
        }}
      }} else {{ $null }}
      sevenDay = if ($data.rate_limits -and $data.rate_limits.seven_day) {{
        @{{
          usedPercentage = $data.rate_limits.seven_day.used_percentage
          resetsAt = $data.rate_limits.seven_day.resets_at
        }}
      }} else {{ $null }}
    }}
    $tmp = "$cachePath.tmp"
    ($out | ConvertTo-Json -Depth 5 -Compress) | Set-Content -Path $tmp -Encoding UTF8
    Move-Item -Path $tmp -Destination $cachePath -Force
  }} catch {{}}
}}
if (Test-Path $prevPath) {{
  try {{
    $prev = Get-Content -Path $prevPath -Raw | ConvertFrom-Json
    if ($prev.command) {{
      $raw | cmd.exe /c $prev.command
      exit $LASTEXITCODE
    }}
  }} catch {{}}
}}
if ($data -and $data.rate_limits -and $data.rate_limits.five_hour -and ($null -ne $data.rate_limits.five_hour.used_percentage)) {{
  $h5 = [Math]::Max(0, [Math]::Round(100 - [double]$data.rate_limits.five_hour.used_percentage))
  $d7 = if ($data.rate_limits.seven_day -and ($null -ne $data.rate_limits.seven_day.used_percentage)) {{
    [Math]::Max(0, [Math]::Round(100 - [double]$data.rate_limits.seven_day.used_percentage))
  }} else {{ $null }}
  if ($null -ne $d7) {{
    Write-Output ("5h " + $h5 + "% left · 7d " + $d7 + "% left")
  }} else {{
    Write-Output ("5h " + $h5 + "% left")
  }}
}}
"#,
            cache = esc_ps(&cache_path),
            prev = esc_ps(&prev_path)
        );
        fs::write(&script_path, script).map_err(|e| e.to_string())?;

        let settings_path = claude_dir.join("settings.json");
        let mut settings: Value = match fs::read_to_string(&settings_path) {
            Ok(raw) => serde_json::from_str(&raw).map_err(|e| format!("Invalid Claude settings JSON: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
            Err(e) => return Err(e.to_string()),
        };
        let obj = settings
            .as_object_mut()
            .ok_or_else(|| "Claude settings root is not a JSON object".to_string())?;

        if let Some(existing) = obj.get("statusLine").cloned() {
            let is_peek = existing
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|c| c.contains("peek-claude-statusline.ps1"));
            if !is_peek && !existing.is_null() {
                crate::preferences::write_json_atomic(
                    &config_dir,
                    "prev_claude_statusline.json",
                    &existing,
                )?;
            }
        }

        let cmd = format!(
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -File \"{}\"",
            script_path.to_string_lossy()
        );
        obj.insert(
            "statusLine".to_string(),
            serde_json::json!({
                "type": "command",
                "command": cmd,
            }),
        );
        crate::preferences::write_json_atomic(&claude_dir, "settings.json", &settings)?;
        Ok(read_all_usage(Some(&config_dir)))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn remove_claude_statusline(app: AppHandle) -> Result<DevMonitorSnapshot, String> {
    let config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let home = std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .map_err(|_| "USERPROFILE not found".to_string())?;
    let claude_dir = home.join(".claude");

    tokio::task::spawn_blocking(move || -> Result<DevMonitorSnapshot, String> {
        let settings_path = claude_dir.join("settings.json");
        if settings_path.exists() {
            let raw = fs::read_to_string(&settings_path).map_err(|e| e.to_string())?;
            let mut settings: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
            if let Some(obj) = settings.as_object_mut() {
                let prev_path = config_dir.join("prev_claude_statusline.json");
                if prev_path.exists() {
                    if let Ok(prev_raw) = fs::read_to_string(&prev_path) {
                        if let Ok(prev_val) = serde_json::from_str::<Value>(&prev_raw) {
                            if !prev_val.is_null() {
                                obj.insert("statusLine".to_string(), prev_val);
                            } else {
                                obj.remove("statusLine");
                            }
                        }
                    }
                    let _ = fs::remove_file(&prev_path);
                } else if obj
                    .get("statusLine")
                    .and_then(|s| s.get("command"))
                    .and_then(Value::as_str)
                    .is_some_and(|c| c.contains("peek-claude-statusline.ps1"))
                {
                    obj.remove("statusLine");
                }
                crate::preferences::write_json_atomic(&claude_dir, "settings.json", &settings)?;
            }
        }
        Ok(read_all_usage(Some(&config_dir)))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_jsonl_deduped_usage_and_status() {
        let lines = vec![
            r#"{"type":"user","cwd":"C:\\Projects\\Peek","gitBranch":"main","sessionId":"s-1"}"#
                .to_string(),
            r#"{"type":"assistant","timestamp":"2026-11-21T05:00:00Z","sessionId":"s-1","message":{"id":"m1","model":"claude-sonnet-4-5","stop_reason":"tool_use","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":400,"cache_creation_input_tokens":0}}}"#
                .to_string(),
            r#"{"type":"assistant","timestamp":"2026-11-21T05:00:02Z","sessionId":"s-1","message":{"id":"m1","model":"claude-sonnet-4-5","stop_reason":"end_turn","usage":{"input_tokens":100,"output_tokens":80,"cache_read_input_tokens":400,"cache_creation_input_tokens":0}}}"#
                .to_string(),
        ];
        let parsed = parse_claude_jsonl_lines(&lines, 1_000_000, &HashSet::new(), 1_010_000)
            .expect("claude parsed");
        assert_eq!(parsed.status, Some(AgentStatus::Completed));
        assert_eq!(parsed.messages.len(), 1);
        let (_, mdl, inp, out, cr, cw) = parsed.messages.values().next().unwrap();
        assert_eq!(inp, &100);
        assert_eq!(out, &80);
        assert_eq!(cr, &400);
        assert!(estimate_claude_cost(mdl, *inp, *out, *cr, *cw).is_some());
    }

    #[test]
    fn parses_codex_token_count_and_real_rate_limits() {
        let lines = vec![
            r#"{"type":"turn_context","payload":{"model":"gpt-5-codex"}}"#.to_string(),
            r#"{"type":"event_msg","timestamp":"2026-11-21T06:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":2000,"cached_input_tokens":1200,"output_tokens":350,"total_tokens":3550},"last_token_usage":{"input_tokens":500,"cached_input_tokens":300,"output_tokens":50,"total_tokens":850}},"rate_limits":{"plan_type":"pro","primary":{"used_percent":42.5,"window_minutes":300,"resets_at":1800000000}}}}"#
                .to_string(),
            r#"{"type":"event_msg","payload":{"type":"exec_approval_request"}}"#.to_string(),
        ];
        let day_start = parse_iso_utc_ms("2026-11-21T00:00:00Z").unwrap();
        let day_end = day_start + 86_400_000;
        let parsed = parse_codex_jsonl_lines(
            &lines,
            day_start + 1000,
            day_start,
            day_end,
            day_start + 5000,
        )
        .unwrap();
        assert_eq!(parsed.status, Some(AgentStatus::NeedsApproval));
        assert_eq!(parsed.active_model, "gpt-5-codex");
        assert_eq!(parsed.total_tokens, 3550);
        assert_eq!(parsed.today_tokens, 850);
        assert_eq!(parsed.rate_limits.len(), 1);
        assert_eq!(parsed.rate_limits[0].label, "5-hour limit");
        assert!((parsed.rate_limits[0].used_percent - 42.5).abs() < 1e-6);
        assert_eq!(parsed.rate_limits[0].resets_at, Some(1_800_000_000));
    }

    #[test]
    fn buckets_5h_and_7d_rolling_windows_with_reset_times() {
        let now_ms = 1_800_000_000_000u64;
        let events = vec![
            (now_ms - 10 * 86_400 * 1000, 999), // older than 7d: ignored
            (now_ms - 3 * 86_400 * 1000, 400),  // inside 7d only
            (now_ms - 2 * 3_600 * 1000, 150),   // inside 5h and 7d
            (now_ms - 30 * 60 * 1000, 50),      // inside 5h and 7d
        ];
        let (h5, d7, h5_reset, d7_reset) = accumulate_window_tokens(events, now_ms);
        assert_eq!(h5, 200);
        assert_eq!(d7, 600);
        assert_eq!(
            h5_reset,
            Some((now_ms - 2 * 3_600 * 1000 + FIVE_HOURS_MS) / 1000)
        );
        assert_eq!(
            d7_reset,
            Some((now_ms - 3 * 86_400 * 1000 + SEVEN_DAYS_MS) / 1000)
        );
    }

    #[test]
    fn parses_antigravity_transcript_usage() {
        let lines = vec![
            r#"{"step_index":1,"type":"USER_INPUT","content":"hello"}"#,
            r#"{"step_index":2,"type":"PLANNER_RESPONSE","created_at":"2026-11-21T05:32:15Z","model":"gemini-4-neon-high","input_tokens":17869,"cache_read_tokens":0,"output_tokens":260}"#,
            r#"{"step_index":3,"type":"PLANNER_RESPONSE","created_at":"2026-11-21T05:32:27Z","model":"gemini-4-neon-high","input_tokens":3534,"cache_read_tokens":15820,"output_tokens":281}"#,
        ];
        let parsed = parse_antigravity_transcript_lines(lines, 1_795_240_000_000).unwrap();
        assert_eq!(parsed.active_model, "gemini-4-neon-high");
        assert_eq!(parsed.entries.len(), 2);
        let total_in: u64 = parsed.entries.iter().map(|e| e.1).sum();
        let total_cache: u64 = parsed.entries.iter().map(|e| e.2).sum();
        let total_out: u64 = parsed.entries.iter().map(|e| e.3).sum();
        assert_eq!(total_in, 21403);
        assert_eq!(total_cache, 15820);
        assert_eq!(total_out, 541);
    }

    #[test]
    fn parses_claude_quota_cache() {
        let raw = r#"{"model":"Claude Sonnet 4.6","fiveHour":{"usedPercentage":23.0,"resetsAt":2000000000},"sevenDay":{"usedPercentage":8.0,"resetsAt":1900000000}}"#;
        let (windows, model) = parse_claude_quota_cache(raw, 1_950_000_000).unwrap();
        assert_eq!(model, "Claude Sonnet 4.6");
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "5-hour limit");
        assert!((windows[0].used_percent - 23.0).abs() < 1e-6);
        assert_eq!(windows[1].label, "Weekly limit");
        // sevenDay reset (1.9B) is in the past relative to 1.95B -> rolls over to 0% used
        assert!((windows[1].used_percent - 0.0).abs() < 1e-6);

        assert!(parse_claude_quota_cache("not json", 1_950_000_000).is_none());
        assert!(parse_claude_quota_cache("{}", 1_950_000_000).is_none());
    }
}
