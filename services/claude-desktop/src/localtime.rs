// AuthManager 新增（cc-same 里没有这个文件）：本地时间的格式化与解析，代替 chrono。
//! Local time without chrono: `localtime_r` and `strftime` to write a time, `mktime` to read
//! one back.

use std::ffi::CStr;
use std::sync::Once;

unsafe extern "C" {
    fn tzset();
}

/// Read the time zone once; `localtime_r` need not do it itself.
fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe { tzset() });
}

/// `secs` (Unix seconds) in local time, written the way `strftime`'s `fmt` says.
pub fn format(secs: i64, fmt: &CStr) -> String {
    init();
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return String::new();
    }
    let mut buf = [0u8; 64];
    let n = unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr(), &tm) };
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// Now, in local time.
pub fn now(fmt: &CStr) -> String {
    format(crate::fsx::now_secs() as i64, fmt)
}

/// A local `YYYY-MM-DD HH:MM:SS` as Unix seconds. `None` for a time that is not valid, including
/// one skipped by a daylight saving change.
pub fn parse_local(stamp: &str) -> Option<f64> {
    let b = stamp.as_bytes();
    if b.len() != 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b' ' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i32> {
        let part = stamp.get(from..to)?;
        part.bytes().all(|c| c.is_ascii_digit()).then(|| part.parse().ok())?
    };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    init();
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month - 1;
    tm.tm_mday = day;
    tm.tm_hour = hour;
    tm.tm_min = minute;
    tm.tm_sec = second;
    tm.tm_isdst = -1;
    let t = unsafe { libc::mktime(&mut tm) };
    // mktime moves an impossible date (Feb 30) or a skipped hour along; such a stamp is not valid.
    let same = tm.tm_year == year - 1900
        && tm.tm_mon == month - 1
        && tm.tm_mday == day
        && tm.tm_hour == hour
        && tm.tm_min == minute
        && tm.tm_sec == second;
    (t != -1 && same).then_some(t as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_time_reads_back_as_it_was_written() {
        let t = 1_790_000_000;
        let text = format(t, c"%Y-%m-%d %H:%M:%S");
        assert_eq!(parse_local(&text), Some(t as f64));
        assert_eq!(format(t, c"%Y%m%d-%H%M%S").len(), 15);
        assert_eq!(parse_local("2026-02-30 10:00:00"), None);
        assert_eq!(parse_local("2026-09-30T10:00:00"), None);
        assert_eq!(parse_local("2026-9-30 10:00:00"), None);
    }
}
