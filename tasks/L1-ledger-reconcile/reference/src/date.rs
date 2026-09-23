//! Calendar dates.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Date {
    /// Parses `YYYY-MM-DD`.
    pub fn parse_iso(s: &str) -> Option<Date> {
        let mut it = s.trim().split('-');
        let year = it.next()?.parse().ok()?;
        let month = it.next()?.parse().ok()?;
        let day = it.next()?.parse().ok()?;
        if it.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        Some(Date { year, month, day })
    }

    /// `YYYY-MM`.
    pub fn month_key(&self) -> String {
        format!("{:04}-{:02}", self.year, self.month)
    }
}
