//! The session log: one line per finished session, so playtime can be shown
//! by day, week, month and year. Older playtime only exists as per-game
//! totals in stats.json ("before tracking started").

use crate::*;
use std::io::Write;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Session {
    pub id: String,
    /// Unix time it started.
    pub start: u64,
    pub secs: u64,
}

fn log_path(dir: &Path) -> PathBuf {
    dir.join("sessions.jsonl")
}

/// Add a session to the log (the caller holds the stats lock).
pub fn append(dir: &Path, s: &Session) {
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(log_path(dir)) {
        let _ = writeln!(f, "{}", serde_json::to_string(s).unwrap_or_default());
    }
}

pub fn load_in(dir: &Path) -> Vec<Session> {
    fs::read_to_string(log_path(dir))
        .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default()
}

pub fn load() -> Vec<Session> {
    load_in(&data_dir())
}

/// Seconds east of UTC in local time at `ts`.
fn utc_offset(ts: u64) -> i64 {
    // SAFETY: localtime_r only writes the struct we pass.
    unsafe {
        let t = ts as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() { 0 } else { tm.tm_gmtoff }
    }
}

/// The local day `ts` falls on, counted from 1970-01-01.
pub fn day_of(ts: u64) -> i64 {
    (ts as i64 + utc_offset(ts)).div_euclid(86_400)
}

/// The local hour (0–23) at `ts`.
pub fn hour_of(ts: u64) -> u32 {
    ((ts as i64 + utc_offset(ts)).rem_euclid(86_400) / 3600) as u32
}

/// (year, month 1–12, day 1–31) of a day number.
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The day number of a date.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// 0 = Monday … 6 = Sunday.
pub fn weekday(days: i64) -> usize {
    (days + 3).rem_euclid(7) as usize
}

/// The first day of the month after the one `days` is in.
pub fn next_month(days: i64) -> i64 {
    let (y, m, _) = civil(days);
    if m == 12 { days_from_civil(y + 1, 1, 1) } else { days_from_civil(y, m + 1, 1) }
}

pub const MONTHS: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
pub const MONTHS_SHORT: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
pub const DAYS_SHORT: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// "Sun 18 Oct" (logical text).
pub fn day_label(days: i64) -> String {
    let (_, m, d) = civil(days);
    lfmt("{} {} {}", &[&lg(DAYS_SHORT[weekday(days)]), &d, &lg(MONTHS_SHORT[m as usize - 1])])
}

/// Seconds played on each local day.
pub fn by_day(sessions: &[Session]) -> HashMap<i64, u64> {
    let mut out = HashMap::new();
    for s in sessions {
        *out.entry(day_of(s.start)).or_insert(0) += s.secs;
    }
    out
}

/// Days in a row with play, ending today (or yesterday: tonight's session
/// shouldn't be needed to keep a streak alive in the morning).
pub fn current_streak(days: &HashMap<i64, u64>, today: i64) -> u32 {
    let mut d = if days.contains_key(&today) { today } else { today - 1 };
    let mut n = 0;
    while days.contains_key(&d) {
        n += 1;
        d -= 1;
    }
    n
}

pub fn longest_streak(days: &HashMap<i64, u64>) -> u32 {
    let mut keys: Vec<i64> = days.keys().copied().collect();
    keys.sort_unstable();
    let (mut best, mut run, mut prev) = (0, 0, i64::MIN);
    for d in keys {
        run = if d == prev + 1 { run + 1 } else { 1 };
        best = best.max(run);
        prev = d;
    }
    best
}

/// What a month or a year looked like.
pub struct Recap {
    pub secs: u64,
    pub sessions: usize,
    /// Games by time played, most first.
    pub games: Vec<(String, u64)>,
    pub best_day: (i64, u64),
    pub longest: Session,
    /// Games first played in this period.
    pub new_games: usize,
}

/// Sessions starting on days `from..to`, summed up. None when there are none.
pub fn recap(sessions: &[Session], from: i64, to: i64) -> Option<Recap> {
    let inside: Vec<&Session> = sessions.iter().filter(|s| (from..to).contains(&day_of(s.start))).collect();
    let longest = (*inside.iter().max_by_key(|s| s.secs)?).clone();
    let mut games: HashMap<&str, u64> = HashMap::new();
    let mut days: HashMap<i64, u64> = HashMap::new();
    for s in &inside {
        *games.entry(&s.id).or_insert(0) += s.secs;
        *days.entry(day_of(s.start)).or_insert(0) += s.secs;
    }
    let first_seen = |id: &str| sessions.iter().filter(|s| s.id == id).map(|s| day_of(s.start)).min();
    let new_games = games.keys().filter(|id| first_seen(id).is_some_and(|d| d >= from)).count();
    let mut games: Vec<(String, u64)> = games.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    games.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let best_day = days.into_iter().max_by_key(|&(d, s)| (s, -d))?;
    Some(Recap { secs: inside.iter().map(|s| s.secs).sum(), sessions: inside.len(), games, best_day, longest, new_games })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip() {
        for d in [-1000, 0, 19_000, 20_744, 30_000] {
            let (y, m, dd) = civil(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
        // 2026-10-18 was a Sunday.
        let d = days_from_civil(2026, 10, 18);
        assert_eq!(weekday(d), 6);
        assert_eq!(civil(next_month(d)), (2026, 11, 1));
        assert_eq!(civil(next_month(days_from_civil(2026, 12, 5))), (2027, 1, 1));
    }

    #[test]
    fn streaks_and_recaps() {
        let days: HashMap<i64, u64> = [(10, 60), (11, 60), (12, 60), (20, 60)].into_iter().collect();
        assert_eq!(current_streak(&days, 12), 3);
        assert_eq!(current_streak(&days, 13), 3);
        assert_eq!(current_streak(&days, 14), 0);
        assert_eq!(longest_streak(&days), 3);
        let at = |day: i64, h: u64| day as u64 * 86_400 + h * 3600 - utc_offset(day as u64 * 86_400) as u64;
        let s = |id: &str, day: i64, secs: u64| Session { id: id.into(), start: at(day, 12), secs };
        let log = vec![s("a", 5, 100), s("a", 40, 300), s("b", 41, 900), s("b", 41, 50)];
        let r = recap(&log, 35, 60).unwrap();
        assert_eq!(r.secs, 1250);
        assert_eq!(r.sessions, 3);
        assert_eq!(r.games[0], ("b".to_string(), 950));
        assert_eq!(r.best_day, (41, 950));
        assert_eq!(r.longest.secs, 900);
        assert_eq!(r.new_games, 1, "only b was first played in the period");
        assert!(recap(&log, 100, 200).is_none());
    }
}
