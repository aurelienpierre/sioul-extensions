// SPDX-License-Identifier: GPL-3.0-or-later

//! An iCalendar writer (RFC 5545) for the agenda an extension hands back:
//! events only, times in UTC, text escaped, lines folded at 75 octets,
//! lines ended by CRLF. What it writes depends only on what it is given, so
//! that the same lessons make the same bytes on every device (Sioul writes
//! nothing when nothing changed).

/// One event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Unique in the agenda, stable from one refresh to the next.
    pub uid: String,
    /// Seconds since 1970, UTC.
    pub start: i64,
    pub end: i64,
    pub summary: String,
    pub location: Option<String>,
    pub description: Option<String>,
    /// `STATUS:CANCELLED`.
    pub cancelled: bool,
    /// `TRANSP:TRANSPARENT`: the time it takes is not the person's own (a child's lesson).
    pub transparent: bool,
    /// A whole-day event, written as floating `DATE` values (`DTSTART;VALUE=DATE`,
    /// `DTEND;VALUE=DATE` the day after the last), no zone. Then `start` and
    /// `end` are UTC midnight of the civil date(s) (`days(y, m, d) * 86_400`):
    /// the day is read from them, never a wall-clock time.
    pub all_day: bool,
}

/// An agenda's events, and the program that wrote them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Calendar {
    /// `PRODID`'s product: the extension and its version.
    pub product: String,
    pub name: String,
    pub events: Vec<Event>,
}

impl Calendar {
    /// The calendar as iCalendar text. `DTSTAMP` is the event's own start, so that it never changes by itself.
    pub fn text(&self) -> String {
        let mut out = String::new();
        let mut line = |text: String| fold(&text, &mut out);
        line("BEGIN:VCALENDAR".into());
        line("VERSION:2.0".into());
        line(format!("PRODID:-//Sioul extensions//{}//EN", escape(&self.product)));
        line("CALSCALE:GREGORIAN".into());
        line(format!("X-WR-CALNAME:{}", escape(&self.name)));
        for event in &self.events {
            line("BEGIN:VEVENT".into());
            line(format!("UID:{}", escape(&event.uid)));
            line(format!("DTSTAMP:{}", stamp(event.start)));
            if event.all_day {
                // Floating DATE values; DTEND is the day after the last (RFC 5545 §3.6.1).
                let first = event.start.div_euclid(86_400);
                let after = event.end.max(event.start + 86_400).div_euclid(86_400).max(first + 1);
                line(format!("DTSTART;VALUE=DATE:{}", date_stamp(first)));
                line(format!("DTEND;VALUE=DATE:{}", date_stamp(after)));
            } else {
                line(format!("DTSTART:{}", stamp(event.start)));
                line(format!("DTEND:{}", stamp(event.end.max(event.start))));
            }
            line(format!("SUMMARY:{}", escape(&event.summary)));
            if let Some(location) = event.location.as_deref().filter(|l| !l.trim().is_empty()) {
                line(format!("LOCATION:{}", escape(location)));
            }
            if let Some(description) = event.description.as_deref().filter(|d| !d.trim().is_empty()) {
                line(format!("DESCRIPTION:{}", escape(description)));
            }
            line(format!("STATUS:{}", if event.cancelled { "CANCELLED" } else { "CONFIRMED" }));
            if event.transparent {
                line("TRANSP:TRANSPARENT".into());
            }
            line("END:VEVENT".into());
        }
        line("END:VCALENDAR".into());
        out
    }
}

/// A text value escaped (RFC 5545 §3.3.11): `\`, `;`, `,` and new lines.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// A content line folded at 75 octets, never inside a character (§3.1), ended by CRLF.
fn fold(line: &str, out: &mut String) {
    let mut room = 75;
    for c in line.chars() {
        let size = c.len_utf8();
        if size > room {
            out.push_str("\r\n ");
            room = 74;
        }
        out.push(c);
        room -= size;
    }
    out.push_str("\r\n");
}

/// Days since 1970-01-01 as a civil date (year, month, day), proleptic Gregorian.
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// A civil date as days since 1970-01-01.
pub fn days(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Days since 1970 as a floating `DATE` value `YYYYMMDD` (no zone).
pub fn date_stamp(days_since_epoch: i64) -> String {
    let (y, m, d) = civil(days_since_epoch);
    format!("{y:04}{m:02}{d:02}")
}

/// Seconds since 1970 as `YYYYMMDDTHHMMSSZ`.
pub fn stamp(at: i64) -> String {
    let (y, m, d) = civil(at.div_euclid(86_400));
    let s = at.rem_euclid(86_400);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

/// Seconds since 1970 as RFC 3339 in UTC (`2026-11-20T07:05:00Z`).
pub fn rfc3339(at: i64) -> String {
    let (y, m, d) = civil(at.div_euclid(86_400));
    let s = at.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_calendar_is_written_as_the_standard_says() {
        let calendar = Calendar {
            product: "test 0.1.0".into(),
            name: "Timetable: Alex".into(),
            events: vec![Event {
                uid: "L-1@test".into(),
                start: 1_795_075_200,
                end: 1_795_078_800,
                summary: "Histoire, géographie; et le reste".into(),
                location: Some("Salle 12".into()),
                description: Some(format!("M. Martin\n{}", "é".repeat(80))),
                cancelled: true,
                transparent: true,
                all_day: false,
            }],
        };
        let text = calendar.text();
        assert!(text.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"));
        assert!(text.contains("DTSTART:20261119T080000Z\r\nDTEND:20261119T090000Z\r\n"));
        assert!(text.contains("SUMMARY:Histoire\\, géographie\\; et le reste\r\n"));
        assert!(text.contains("STATUS:CANCELLED\r\nTRANSP:TRANSPARENT\r\n"));
        for line in text.split("\r\n") {
            assert!(line.len() <= 75, "{line}");
        }
        let unfolded = text.replace("\r\n ", "");
        assert!(unfolded.contains(&format!("DESCRIPTION:M. Martin\\n{}\r\n", "é".repeat(80))));
        assert!(text.ends_with("END:VCALENDAR\r\n"));
        assert_eq!(calendar.text(), text, "the same events, the same bytes");
    }

    #[test]
    fn an_all_day_event_is_written_as_floating_dates() {
        // start and end are UTC midnight of the civil date; the day is read from them.
        let day = days(2026, 11, 25) * 86_400;
        let calendar = Calendar {
            product: "test 0.1.0".into(),
            name: "Planity".into(),
            events: vec![Event { uid: "B-1".into(), start: day, end: day + 86_400, summary: "Unavailable".into(), location: None, description: None, cancelled: false, transparent: false, all_day: true }],
        };
        let text = calendar.text();
        assert!(text.contains("DTSTART;VALUE=DATE:20261125\r\n"), "{text}");
        assert!(text.contains("DTEND;VALUE=DATE:20261126\r\n"), "the day after the last: {text}");
        assert!(text.contains("DTSTAMP:20261125T000000Z\r\n"), "{text}");
        assert!(!text.contains("TRANSP:"), "a busy event writes no TRANSP; the door decides: {text}");
    }

    #[test]
    fn dates_go_both_ways() {
        for day in [-1, 0, 59, 10_957, 20_775, 20_776, 47_482] {
            let (y, m, d) = civil(day);
            assert_eq!(days(y, m, d), day);
        }
        assert_eq!(civil(20_775), (2026, 11, 18));
        assert_eq!(rfc3339(1_795_046_400), "2026-11-19T00:00:00Z");
        assert_eq!(stamp(951_782_400), "20000229T000000Z");
    }
}
