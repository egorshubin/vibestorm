//! Account-wide plan usage, the same figures Claude Code's `/usage` shows: 5-hour session,
//! weekly limits (all models and per model), extra usage. Counts every client on the
//! account (other terminals, IDEs, claude.ai), not just our agents.
//!
//! Read from `api.anthropic.com/api/oauth/usage` with Claude Code's own OAuth token
//! (`~/.claude/.credentials.json`). The token is never refreshed here; Claude Code does
//! that itself the next time it runs.

use crate::claude::RateLimit;
use eframe::egui;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const URL: &str = "https://api.anthropic.com/api/oauth/usage";

#[derive(Default)]
pub struct PlanState {
    pub limits: Vec<RateLimit>,
    pub error: Option<String>,
    pub fetched: Option<Instant>,
}

pub struct PlanUsage {
    pub state: Arc<Mutex<PlanState>>,
    busy: Arc<Mutex<bool>>,
    last_try: Option<Instant>,
}

impl PlanUsage {
    pub fn new() -> Self {
        PlanUsage { state: Arc::default(), busy: Arc::default(), last_try: None }
    }

    pub fn busy(&self) -> bool {
        *self.busy.lock().unwrap()
    }

    /// Fetch again if the last attempt is older than `max_age`.
    pub fn refresh_if_older(&mut self, max_age: Duration, ctx: &egui::Context) {
        if self.last_try.is_none_or(|t| t.elapsed() >= max_age) {
            self.refresh(ctx);
        }
    }

    pub fn refresh(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        self.last_try = Some(Instant::now());
        *self.busy.lock().unwrap() = true;
        let (state, busy, ctx) = (self.state.clone(), self.busy.clone(), ctx.clone());
        std::thread::spawn(move || {
            let res = fetch();
            {
                let mut s = state.lock().unwrap();
                match res {
                    Ok(limits) => {
                        s.limits = limits;
                        s.error = None;
                        s.fetched = Some(Instant::now());
                    }
                    Err(e) => s.error = Some(e),
                }
            }
            *busy.lock().unwrap() = false;
            ctx.request_repaint();
        });
    }
}

fn credentials_path() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(d).join(".credentials.json"));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".claude").join(".credentials.json"))
}

fn fetch() -> Result<Vec<RateLimit>, String> {
    let path = credentials_path().ok_or("no home folder")?;
    let text = std::fs::read_to_string(&path).map_err(|_| "not signed in to Claude Code (no ~/.claude/.credentials.json)".to_owned())?;
    let creds: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("bad credentials file: {e}"))?;
    let oauth = &creds["claudeAiOauth"];
    let token = oauth["accessToken"].as_str().ok_or("Claude Code is not signed in with a Claude account")?;
    if oauth["expiresAt"].as_u64().is_some_and(|ms| ms / 1000 <= crate::util::unix_now()) {
        return Err("sign-in expired; it renews the next time Claude runs".into());
    }

    // curl.exe ships with Windows 10+. The token goes in via stdin (`-H @-`),
    // so it never shows up in a process command line.
    let mut child = crate::util::hidden_cmd("curl")
        .args(["-sS", "--max-time", "20", "-H", "@-", "-H", "anthropic-beta: oauth-2025-04-20", "-H", "Accept: application/json"])
        .args(["-A", concat!("vibestorm/", env!("CARGO_PKG_VERSION")), "-w", "\n%{http_code}", URL])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "Authorization: Bearer {token}");
    }
    let out = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let (body, code) = stdout.trim_end().rsplit_once('\n').unwrap_or(("", stdout.trim()));
    match code.trim() {
        "200" => parse(body),
        "000" | "" => Err(format!("network error: {}", String::from_utf8_lossy(&out.stderr).trim())),
        "401" => Err("sign-in expired; it renews the next time Claude runs".into()),
        c => Err(format!("HTTP {c}: {}", crate::util::truncate(body.trim(), 160))),
    }
}

/// Every top-level `{utilization, resets_at}` entry becomes a limit, so new ones
/// (e.g. a weekly limit for a new model) show up without code changes.
fn parse(body: &str) -> Result<Vec<RateLimit>, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("bad response: {e}"))?;
    let obj = v.as_object().ok_or("bad response")?;
    let now = Instant::now();
    let mut out = vec![];
    for (key, e) in obj {
        let Some(e) = e.as_object() else { continue };
        if !e.contains_key("utilization") {
            continue;
        }
        if e.get("is_enabled").is_some_and(|b| b == false) {
            continue; // extra usage switched off
        }
        let utilization = e["utilization"].as_f64().map(|u| u / 100.0);
        let resets_at = e.get("resets_at").and_then(|r| r.as_str()).and_then(parse_iso8601);
        if utilization.is_none() && resets_at.is_none() {
            continue;
        }
        let kind = if key == "extra_usage" { "overage".to_owned() } else { key.clone() };
        let status = match utilization {
            Some(u) if u >= 1.0 => "rejected",
            Some(u) if u >= 0.8 => "allowed_warning",
            _ => "allowed",
        };
        out.push(RateLimit { kind, status: status.into(), resets_at, utilization, at: now });
    }
    Ok(out)
}

/// "2026-11-18T14:59:59.943648+00:00" → unix secs.
fn parse_iso8601(s: &str) -> Option<u64> {
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    // timezone: Z or ±hh:mm after the optional fraction
    let tail = &s[19..];
    let tz = tail.find(['Z', '+', '-']).map(|i| &tail[i..]).unwrap_or("Z");
    let offset = match tz.as_bytes()[0] {
        b'Z' => 0,
        sign => {
            let h: i64 = tz.get(1..3)?.parse().ok()?;
            let m: i64 = tz.get(4..6).and_then(|m| m.parse().ok()).unwrap_or(0);
            (h * 3600 + m * 60) * if sign == b'-' { -1 } else { 1 }
        }
    };
    // days from civil (Howard Hinnant)
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((mo + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + h * 3600 + mi * 60 + sec - offset).ok()
}

/// Display name for a limit kind: five_hour, seven_day, seven_day_<model>, overage, ...
pub fn label(kind: &str) -> String {
    let cap = |s: &str| {
        let s = s.replace('_', " ");
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    };
    match kind {
        "five_hour" => "Current session (5 h)".into(),
        "seven_day" => "Week · all models".into(),
        "seven_day_oauth_apps" => "Week · OAuth apps".into(),
        "overage" | "extra_usage" => "Extra usage".into(),
        k => match k.strip_prefix("seven_day_") {
            Some(m) => format!("Week · {}", cap(m)),
            None => cap(k),
        },
    }
}

/// Sort key: session first, then the weekly total, then per-model weeks, then the rest.
pub fn order(kind: &str) -> (u8, String) {
    let rank = match kind {
        "five_hour" => 0,
        "seven_day" => 1,
        k if k.starts_with("seven_day_") => 2,
        "overage" => 4,
        _ => 3,
    };
    (rank, kind.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_usage_response() {
        let body = r#"{
            "five_hour": {"utilization": 12.0, "resets_at": "2026-11-18T14:59:59.943648+00:00"},
            "seven_day": {"utilization": 35.5, "resets_at": "2026-11-20T09:00:00+00:00"},
            "seven_day_oauth_apps": null,
            "seven_day_opus": {"utilization": 0.0, "resets_at": null},
            "seven_day_fable": {"utilization": 81.0, "resets_at": "2026-11-20T09:00:00Z"},
            "extra_usage": {"is_enabled": false, "monthly_limit": null, "used_credits": null, "utilization": null}
        }"#;
        let l = parse(body).unwrap();
        let get = |k: &str| l.iter().find(|r| r.kind == k);
        assert_eq!(l.len(), 4);
        assert_eq!(get("five_hour").unwrap().utilization, Some(0.12));
        assert_eq!(get("five_hour").unwrap().resets_at, Some(1_795_013_999));
        assert_eq!(get("seven_day_fable").unwrap().status, "allowed_warning");
        assert_eq!(get("seven_day_opus").unwrap().resets_at, None);
        assert!(get("overage").is_none());
        assert_eq!(label("seven_day_fable"), "Week · Fable");
    }

    #[test]
    fn iso8601_offsets() {
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601("1970-01-01T03:00:00+03:00"), Some(0));
        assert_eq!(parse_iso8601("2000-03-01T00:00:00.5-01:30"), Some(951_868_800 + 5400));
    }
}
