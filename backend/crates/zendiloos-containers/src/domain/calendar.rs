//! The host's local calendar, for schedules people set ("at three at
//! night"): dates, weekdays, and a local time of a date as a moment, with
//! daylight saving time (each date's time with the offset of that date, the
//! first time the clock shows it where it shows it twice, the time after
//! where it skips it).

use zendiloos_kernel::{Timestamp, UtcTime};

const DAY: i64 = 24 * 3600;

/// How far the host's clock is ahead of UTC at a moment, in seconds.
pub type UtcOffsets<'a> = &'a dyn Fn(Timestamp) -> i32;

/// `at` in the host's local time, as calendar fields.
pub(crate) fn local(at: Timestamp, offset: i32) -> UtcTime {
    Timestamp::from_unix_seconds(at.unix_seconds() + i64::from(offset)).utc()
}

/// The days since 1 January 1970 of a date.
pub(crate) fn day_number(time: UtcTime) -> i64 {
    let midnight = UtcTime { hour: 0, minute: 0, second: 0, ..time };
    midnight.timestamp().map_or(0, Timestamp::unix_seconds).div_euclid(DAY)
}

/// The date `days` after 1 January 1970.
pub(crate) fn date(days: i64) -> UtcTime {
    Timestamp::from_unix_seconds(days * DAY).utc()
}

/// 0 is Monday: 1 January 1970 was a Thursday.
pub(crate) fn weekday(time: UtcTime) -> i64 {
    (day_number(time) + 3).rem_euclid(7)
}

pub(crate) fn days_in(year: i64, month: u8) -> u8 {
    [31, 30, 29, 28]
        .into_iter()
        .find(|&day| UtcTime { year, month, day, hour: 0, minute: 0, second: 0 }.timestamp().is_some())
        .unwrap_or(28)
}

/// The month before or after.
pub(crate) fn month_beside(year: i64, month: u8, later: bool) -> (i64, u8) {
    match (month, later) {
        (1, false) => (year - 1, 12),
        (12, true) => (year + 1, 1),
        (month, false) => (year, month - 1),
        (month, true) => (year, month + 1),
    }
}

/// `hour:minute` local time on a date, as a moment, with the offset the
/// clock has then: where it shows that time twice (summer time ends), the
/// first; where it skips it (summer time begins), the time after. A day the
/// month has not is its last.
pub(crate) fn moment(
    year: i64,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    offsets: UtcOffsets<'_>,
) -> Timestamp {
    let day = day.min(days_in(year, month));
    let local = UtcTime { year, month, day, hour: hour.min(23), minute: minute.min(59), second: 0 };
    let seconds = local.timestamp().map_or(0, Timestamp::unix_seconds);
    // The offsets before and after any change near that time: none is
    // more than fourteen hours from UTC.
    let around = |shift: i64| offsets(Timestamp::from_unix_seconds(seconds + shift));
    let (before, after) = (around(-15 * 3600), around(15 * 3600));
    let at = |offset: i32| seconds - i64::from(offset);
    let shown = [before, after]
        .into_iter()
        .filter(|offset| offsets(Timestamp::from_unix_seconds(at(*offset))) == *offset);
    Timestamp::from_unix_seconds(shown.map(at).min().unwrap_or_else(|| at(before)))
}
