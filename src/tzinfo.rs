//! Time zones without extra crates: UTC offsets read from the system's TZif files
//! (/usr/share/zoneinfo), including the POSIX TZ rule at their end that newer ("slim") files rely on
//! for current dates. Used for the additional clocks (Date and Time, the taskbar clock) and the
//! "(UTC+01:00)" names and daylight saving notes in Control Panel.
use std::path::PathBuf;

pub fn zoneinfo_dir() -> PathBuf {
    std::env::var_os("TZDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/usr/share/zoneinfo"))
}

/// The system time zone ("Europe/Berlin"), from /etc/localtime.
pub fn local_zone() -> String {
    if let Ok(z) = std::env::var("TZ") { let z = z.trim_start_matches(':'); if !z.is_empty() && !z.starts_with('/') { return z.to_string(); } }
    std::fs::read_link("/etc/localtime").ok()
        .and_then(|p| { let s = p.to_string_lossy().into_owned(); s.split_once("zoneinfo/").map(|(_, z)| z.to_string()) })
        .or_else(|| std::fs::read_to_string("/etc/timezone").ok().map(|s| s.trim().to_string()))
        .unwrap_or_else(|| "UTC".into())
}

#[derive(Debug, Clone, Default)]
pub struct Zone {
    /// transition times (UTC) and the type index that starts at each
    trans: Vec<(i64, usize)>,
    /// (offset seconds east of UTC, is DST)
    types: Vec<(i32, bool)>,
    rule: Option<Rule>,
}

/// A POSIX TZ string ("CET-1CEST,M3.5.0,M10.5.0/3").
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub std_off: i32,
    pub dst: Option<(i32, Date, i32, Date, i32)>, // dst offset, start date, start time, end date, end time
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Date { M(u32, u32, u32), J(u32), N(u32) }

impl Zone {
    pub fn load(name: &str) -> Option<Zone> {
        if name.contains("..") { return None; }
        if name == "UTC" || name == "Etc/UTC" || name.is_empty() {
            if !zoneinfo_dir().join("UTC").exists() { return Some(Zone { rule: Some(Rule { std_off: 0, dst: None }), ..Default::default() }); }
        }
        parse_tzif(&std::fs::read(zoneinfo_dir().join(name)).ok()?)
    }

    /// Seconds east of UTC at `utc` (unix time), and whether that is daylight saving time.
    pub fn offset_at(&self, utc: i64) -> (i32, bool) {
        let last = self.trans.last().map(|t| t.0);
        if let (Some(r), true) = (&self.rule, last.map(|l| utc >= l).unwrap_or(true)) { return r.offset_at(utc); }
        match self.trans.iter().rposition(|t| t.0 <= utc) {
            Some(i) => self.types.get(self.trans[i].1).copied().unwrap_or((0, false)),
            // before the first transition: the first standard-time type
            None => self.types.iter().find(|t| !t.1).or(self.types.first()).copied().unwrap_or((0, false)),
        }
    }

    /// The standard (non daylight saving) offset now: what Windows puts in "(UTC+01:00)".
    pub fn std_offset(&self, utc: i64) -> i32 {
        if let Some(r) = &self.rule { if self.trans.last().map(|l| utc >= l.0).unwrap_or(true) { return r.std_off; } }
        let (o, dst) = self.offset_at(utc);
        if !dst { return o; }
        self.trans.iter().rev().filter(|t| t.0 <= utc).filter_map(|t| self.types.get(t.1)).find(|t| !t.1).map(|t| t.0).unwrap_or(o)
    }

    /// Does the zone change its clocks (within a year from `utc`)?
    pub fn observes_dst(&self, utc: i64) -> bool { self.next_change(utc).is_some() }

    /// The next offset change after `utc` within ~13 months: (UTC time, old offset, new offset).
    pub fn next_change(&self, utc: i64) -> Option<(i64, i32, i32)> {
        let (cur, _) = self.offset_at(utc);
        let mut lo = utc;
        let step = 86_400;
        let mut hi = None;
        for d in 1..=400 {
            let t = utc + d * step;
            if self.offset_at(t).0 != cur { hi = Some(t); break; }
            lo = t;
        }
        let mut hi = hi?;
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if self.offset_at(mid).0 != cur { hi = mid; } else { lo = mid; }
        }
        Some((hi, cur, self.offset_at(hi).0))
    }
}

/// Offset of a zone right now ((0, false) when it cannot be read).
pub fn offset_now(zone: &str) -> i32 {
    Zone::load(zone).map(|z| z.offset_at(now()).0).unwrap_or(0)
}

pub fn now() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }

/// "(UTC+01:00)", "(UTC)" for zero.
pub fn utc_label(off: i32) -> String {
    if off == 0 { return "(UTC)".into(); }
    let s = if off < 0 { '-' } else { '+' };
    let a = off.abs();
    format!("(UTC{s}{:02}:{:02})", a / 3600, a % 3600 / 60)
}

fn be32(b: &[u8], i: usize) -> Option<i32> { Some(i32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?)) }
fn be64(b: &[u8], i: usize) -> Option<i64> { Some(i64::from_be_bytes(b.get(i..i + 8)?.try_into().ok()?)) }

fn parse_tzif(b: &[u8]) -> Option<Zone> {
    if b.get(0..4)? != b"TZif" { return None; }
    let version = b[4];
    let counts = |at: usize| -> Option<[usize; 6]> {
        let mut c = [0usize; 6];
        for (k, v) in c.iter_mut().enumerate() { *v = be32(b, at + 20 + k * 4)? as usize; }
        Some(c)
    };
    // [isutcnt, isstdcnt, leapcnt, timecnt, typecnt, charcnt]
    let c1 = counts(0)?;
    let v1_len = 44 + c1[3] * 5 + c1[4] * 6 + c1[5] + c1[2] * 8 + c1[1] + c1[0];
    let (start, c, tsize) = if version >= b'2' { (v1_len, counts(v1_len)?, 8) } else { (0, c1, 4) };
    let mut p = start + 44;
    let mut trans = Vec::with_capacity(c[3]);
    for i in 0..c[3] { trans.push(if tsize == 8 { be64(b, p + i * 8)? } else { be32(b, p + i * 4)? as i64 }); }
    p += c[3] * tsize;
    let idx: Vec<usize> = b.get(p..p + c[3])?.iter().map(|x| *x as usize).collect();
    p += c[3];
    let mut types = Vec::with_capacity(c[4]);
    for i in 0..c[4] { types.push((be32(b, p + i * 6)?, *b.get(p + i * 6 + 4)? != 0)); }
    p += c[4] * 6 + c[5] + c[2] * (tsize + 4) + c[1] + c[0];
    let rule = if version >= b'2' {
        // footer: "\n<TZ string>\n"
        b.get(p..).and_then(|f| std::str::from_utf8(f).ok()).and_then(|s| s.trim_matches('\n').lines().next().map(|x| x.to_string())).and_then(|s| parse_rule(&s))
    } else { None };
    Some(Zone { trans: trans.into_iter().zip(idx).collect(), types, rule })
}

/// Parse a POSIX TZ string.
pub fn parse_rule(s: &str) -> Option<Rule> {
    let mut it = s.chars().peekable();
    let name = |it: &mut std::iter::Peekable<std::str::Chars>| -> Option<()> {
        if it.peek() == Some(&'<') { for ch in it.by_ref() { if ch == '>' { return Some(()); } } return None; }
        let mut n = 0;
        while it.peek().map(|c| c.is_ascii_alphabetic()).unwrap_or(false) { it.next(); n += 1; }
        (n >= 3).then_some(())
    };
    // [+-]hh[:mm[:ss]] → seconds
    let time = |it: &mut std::iter::Peekable<std::str::Chars>| -> Option<i32> {
        let mut sign = 1;
        if let Some(c) = it.peek().copied() { if c == '+' || c == '-' { if c == '-' { sign = -1; } it.next(); } }
        let mut parts = [0i32; 3];
        let mut k = 0;
        let mut any = false;
        while k < 3 {
            let mut v = 0;
            let mut digits = 0;
            while let Some(d) = it.peek().and_then(|c| c.to_digit(10)) { v = v * 10 + d as i32; it.next(); digits += 1; }
            if digits == 0 { break; }
            any = true;
            parts[k] = v;
            k += 1;
            if it.peek() == Some(&':') { it.next(); } else { break; }
        }
        any.then(|| sign * (parts[0] * 3600 + parts[1] * 60 + parts[2]))
    };
    name(&mut it)?;
    let std_off = -time(&mut it)?;
    if it.peek().is_none() { return Some(Rule { std_off, dst: None }); }
    name(&mut it)?;
    let dst_off = if it.peek().map(|c| *c != ',').unwrap_or(false) { -time(&mut it)? } else { std_off + 3600 };
    let date = |it: &mut std::iter::Peekable<std::str::Chars>| -> Option<(Date, i32)> {
        if it.next()? != ',' { return None; }
        let num = |it: &mut std::iter::Peekable<std::str::Chars>| -> Option<u32> {
            let mut v = 0; let mut n = 0;
            while let Some(d) = it.peek().and_then(|c| c.to_digit(10)) { v = v * 10 + d; it.next(); n += 1; }
            (n > 0).then_some(v)
        };
        let d = match it.peek()? {
            'M' => { it.next(); let m = num(it)?; if it.next()? != '.' { return None; } let w = num(it)?; if it.next()? != '.' { return None; } Date::M(m, w, num(it)?) }
            'J' => { it.next(); Date::J(num(it)?) }
            _ => Date::N(num(it)?),
        };
        let t = if it.peek() == Some(&'/') { it.next(); time(it)? } else { 7200 };
        Some((d, t))
    };
    let (sd, st) = date(&mut it).unwrap_or((Date::M(3, 2, 0), 7200));
    let (ed, et) = date(&mut it).unwrap_or((Date::M(11, 1, 0), 7200));
    Some(Rule { std_off, dst: Some((dst_off, sd, st, ed, et)) })
}

/// Days since 1970-01-01 of a civil date.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn year_of(days: i64) -> i64 {
    // civil_from_days, year only
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    yoe + era * 400 + if m <= 2 { 1 } else { 0 }
}

fn leap(y: i64) -> bool { (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 }

/// Seconds since 1970 (local wall time of that year) when a rule date starts.
fn rule_day(y: i64, d: Date) -> i64 {
    let days = match d {
        Date::J(n) => { let n = n as i64; days_from_civil(y, 1, 1) + n - 1 + if leap(y) && n >= 60 { 1 } else { 0 } }
        Date::N(n) => days_from_civil(y, 1, 1) + n as i64,
        Date::M(m, w, wd) => {
            let first = days_from_civil(y, m, 1);
            let first_wd = (first + 4).rem_euclid(7); // 1970-01-01 was a Thursday
            let mut day = (wd as i64 - first_wd).rem_euclid(7) + (w as i64 - 1) * 7;
            let mdays = [31, if leap(y) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][(m as usize - 1).min(11)];
            while day >= mdays { day -= 7; }
            first + day
        }
    };
    days * 86_400
}

impl Rule {
    pub fn offset_at(&self, utc: i64) -> (i32, bool) {
        let Some((dst_off, sd, st, ed, et)) = self.dst else { return (self.std_off, false) };
        let y = year_of((utc + self.std_off as i64).div_euclid(86_400));
        let start = rule_day(y, sd) + st as i64 - self.std_off as i64;
        let end = rule_day(y, ed) + et as i64 - dst_off as i64;
        let in_dst = if start < end { utc >= start && utc < end } else { !(utc >= end && utc < start) };
        if in_dst { (dst_off, true) } else { (self.std_off, false) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_rules() {
        let r = parse_rule("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        assert_eq!(r.std_off, 3600);
        // 2026-07-01 12:00 UTC: summer time; 2026-12-01: winter
        assert_eq!(r.offset_at(days_from_civil(2026, 7, 1) * 86400 + 43200), (7200, true));
        assert_eq!(r.offset_at(days_from_civil(2026, 12, 1) * 86400), (3600, false));
        // the change: 2026-10-25 01:00 UTC
        let t = days_from_civil(2026, 10, 25) * 86400 + 3600;
        assert_eq!(r.offset_at(t - 1).0, 7200);
        assert_eq!(r.offset_at(t).0, 3600);
        let us = parse_rule("EST5EDT,M3.2.0,M11.1.0").unwrap();
        assert_eq!(us.std_off, -18000);
        assert_eq!(us.offset_at(days_from_civil(2026, 7, 4) * 86400).0, -14400);
        // southern hemisphere
        let au = parse_rule("AEST-10AEDT,M10.1.0,M4.1.0/3").unwrap();
        assert_eq!(au.offset_at(days_from_civil(2026, 1, 15) * 86400).0, 39600);
        assert_eq!(au.offset_at(days_from_civil(2026, 6, 15) * 86400).0, 36000);
        let ist = parse_rule("<+0530>-5:30").unwrap();
        assert_eq!(ist.offset_at(0), (19800, false));
        assert_eq!(utc_label(19800), "(UTC+05:30)");
        assert_eq!(utc_label(-18000), "(UTC-05:00)");
    }

    #[test]
    fn system_zones() {
        // only when the zoneinfo database is installed
        let Some(z) = Zone::load("Europe/Berlin") else { return };
        assert_eq!(z.offset_at(days_from_civil(2026, 7, 1) * 86400).0, 7200);
        assert_eq!(z.std_offset(days_from_civil(2026, 7, 1) * 86400), 3600);
        let (t, from, to) = z.next_change(days_from_civil(2026, 9, 1) * 86400).unwrap();
        assert_eq!((t, from, to), (days_from_civil(2026, 10, 25) * 86400 + 3600, 7200, 3600));
        let k = Zone::load("Asia/Kolkata").unwrap();
        assert_eq!(k.offset_at(now()).0, 19800);
        assert!(!k.observes_dst(now()));
    }
}
