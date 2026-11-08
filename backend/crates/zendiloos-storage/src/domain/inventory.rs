//! The disks the NAS has seen, by serial number, with what the owner noted
//! about each: when and where it was bought, until when the warranty
//! holds. A disk taken out stays listed until it is removed from the list.

use std::fmt;

use zendiloos_kernel::Timestamp;
use zendiloos_kernel::UtcTime;

pub const MAX_VENDOR_CHARS: usize = 64;
pub const MAX_NOTE_CHARS: usize = 500;

/// A day of the calendar, written `YYYY-MM-DD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CalendarDate {
    year: u16,
    month: u8,
    day: u8,
}

impl CalendarDate {
    /// A date from 1970 to 2199 that exists.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('-');
        let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() || year.len() != 4 || month.len() != 2 || day.len() != 2 {
            return None;
        }
        let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
        if !digits(year) || !digits(month) || !digits(day) {
            return None;
        }
        let date = Self { year: year.parse().ok()?, month: month.parse().ok()?, day: day.parse().ok()? };
        let exists = UtcTime {
            year: i64::from(date.year),
            month: date.month,
            day: date.day,
            hour: 0,
            minute: 0,
            second: 0,
        }
        .timestamp()
        .is_some();
        (exists && (1970..2200).contains(&date.year)).then_some(date)
    }
}

impl fmt::Display for CalendarDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// What the owner noted about a disk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InventoryDetails {
    pub purchased: Option<CalendarDate>,
    pub warranty_until: Option<CalendarDate>,
    /// Where it was bought: the vendor or shop.
    pub vendor: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidInventory {
    #[error("a date is written YYYY-MM-DD, from 1970 to 2199")]
    Date,
    #[error("the warranty cannot end before the disk was bought")]
    WarrantyBeforePurchase,
    #[error("the vendor is one line of at most {MAX_VENDOR_CHARS} characters")]
    Vendor,
    #[error("the note has at most {MAX_NOTE_CHARS} characters")]
    Note,
}

impl InventoryDetails {
    /// From what was typed: dates as `YYYY-MM-DD`; blank texts are none.
    pub fn new(
        purchased: Option<&str>,
        warranty_until: Option<&str>,
        vendor: Option<&str>,
        note: Option<&str>,
    ) -> Result<Self, InvalidInventory> {
        let date = |text: Option<&str>| match text.map(str::trim).filter(|t| !t.is_empty()) {
            None => Ok(None),
            Some(text) => CalendarDate::parse(text).map(Some).ok_or(InvalidInventory::Date),
        };
        let (purchased, warranty_until) = (date(purchased)?, date(warranty_until)?);
        if let (Some(bought), Some(until)) = (purchased, warranty_until)
            && until < bought
        {
            return Err(InvalidInventory::WarrantyBeforePurchase);
        }
        let text = |text: Option<&str>| text.map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned);
        let vendor = text(vendor);
        if vendor
            .as_deref()
            .is_some_and(|v| v.chars().count() > MAX_VENDOR_CHARS || v.chars().any(char::is_control))
        {
            return Err(InvalidInventory::Vendor);
        }
        let note = text(note);
        let tab_or_line = |c: char| c == '\n' || c == '\t';
        if note.as_deref().is_some_and(|n| {
            n.chars().count() > MAX_NOTE_CHARS || n.chars().any(|c| c.is_control() && !tab_or_line(c))
        }) {
            return Err(InvalidInventory::Note);
        }
        Ok(Self { purchased, warranty_until, vendor, note })
    }
}

/// A disk the NAS has seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryRecord {
    pub serial: String,
    /// As last seen.
    pub model: Option<String>,
    pub size_bytes: u64,
    pub details: InventoryDetails,
    pub first_seen: Timestamp,
    pub last_seen: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_days_that_exist() {
        let date = CalendarDate::parse("2024-02-29").unwrap();
        assert_eq!(date.to_string(), "2024-02-29");
        assert!(CalendarDate::parse("1970-01-01").is_some());
        assert!(CalendarDate::parse("2199-12-31").is_some());
        for bad in [
            "2023-02-29",
            "2024-13-01",
            "2024-00-10",
            "2024-1-01",
            "24-01-01",
            "2024-01-01-01",
            "2024-01",
            "1969-12-31",
            "2200-01-01",
            "2024-+1-01",
            "abcd-01-01",
            "",
        ] {
            assert_eq!(CalendarDate::parse(bad), None, "{bad:?}");
        }
        assert!(CalendarDate::parse("2023-12-31") < CalendarDate::parse("2024-01-01"));
    }

    #[test]
    fn details_are_checked_and_blanks_are_none() {
        let details = InventoryDetails::new(
            Some("2024-03-01"),
            Some(" 2027-03-01 "),
            Some(" Shop "),
            Some("RMA\nonce"),
        )
        .unwrap();
        assert_eq!(details.purchased.unwrap().to_string(), "2024-03-01");
        assert_eq!(details.warranty_until.unwrap().to_string(), "2027-03-01");
        assert_eq!(details.vendor.as_deref(), Some("Shop"));
        assert_eq!(details.note.as_deref(), Some("RMA\nonce"));
        assert_eq!(
            InventoryDetails::new(Some(""), None, Some("  "), Some("")).unwrap(),
            InventoryDetails::default()
        );

        let invalid = |purchased, until, vendor, note| {
            InventoryDetails::new(purchased, until, vendor, note).unwrap_err()
        };
        assert_eq!(invalid(Some("2024-02-30"), None, None, None), InvalidInventory::Date);
        assert_eq!(invalid(None, Some("soon"), None, None), InvalidInventory::Date);
        assert_eq!(
            invalid(Some("2024-03-01"), Some("2024-02-01"), None, None),
            InvalidInventory::WarrantyBeforePurchase
        );
        let long_vendor = "v".repeat(MAX_VENDOR_CHARS + 1);
        assert_eq!(invalid(None, None, Some(&long_vendor), None), InvalidInventory::Vendor);
        assert_eq!(invalid(None, None, Some("a\nb"), None), InvalidInventory::Vendor);
        let long_note = "n".repeat(MAX_NOTE_CHARS + 1);
        assert_eq!(invalid(None, None, None, Some(&long_note)), InvalidInventory::Note);
        assert_eq!(invalid(None, None, None, Some("bell\u{7}")), InvalidInventory::Note);
    }
}
