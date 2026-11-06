//! Query composition and calendar-date validation. Places/content remain Google's search.
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, NaiveDate};

#[derive(Clone, Default)]
pub struct DateRange {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}
impl DateRange {
    pub fn parse(from: Option<&str>, to: Option<&str>) -> Result<Self> {
        fn date(s: &str) -> Result<NaiveDate> {
            let date = NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .context("Dates must be valid YYYY-MM-DD calendar dates")?;
            if date.format("%Y-%m-%d").to_string() != s {
                bail!("Dates must use YYYY-MM-DD");
            }
            Ok(date)
        }
        let range = Self {
            from: from.map(date).transpose()?,
            to: to.map(date).transpose()?,
        };
        if matches!((range.from,range.to),(Some(a),Some(b)) if a>b) {
            bail!("--from must be on or before --to");
        }
        Ok(range)
    }
    pub fn active(&self) -> bool {
        self.from.is_some() || self.to.is_some()
    }
    pub fn matches(&self, timestamp: Option<&str>) -> Option<bool> {
        if !self.active() {
            return Some(true);
        }
        let date = DateTime::parse_from_rfc3339(timestamp?).ok()?.date_naive();
        Some(self.from.is_none_or(|d| date >= d) && self.to.is_none_or(|d| date <= d))
    }
    /// Start the descending Google timeline after every timezone's end of the selected date.
    pub fn server_before_ms(&self) -> Option<i64> {
        Some(
            (self.to?.succ_opt()?.and_hms_opt(0, 0, 0)?.and_utc() + Duration::hours(14))
                .timestamp_millis(),
        )
    }
    pub fn older_than_range(&self, timestamp: Option<&str>) -> bool {
        let Some(from) = self.from else {
            return false;
        };
        let Some(time) = timestamp.and_then(|s| DateTime::parse_from_rfc3339(s).ok()) else {
            return false;
        };
        time.timestamp_millis()
            < (from.and_hms_opt(0, 0, 0).unwrap().and_utc() - Duration::hours(14))
                .timestamp_millis()
    }
}
pub fn compose_query(query: Option<&str>, location: Option<&str>, tags: &[String]) -> String {
    query
        .into_iter()
        .chain(location)
        .chain(tags.iter().map(String::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dates_are_inclusive_in_each_photos_returned_timezone() {
        let r = DateRange::parse(Some("2026-10-01"), Some("2026-10-31")).unwrap();
        assert_eq!(r.matches(Some("2026-10-01T00:01:00+14:00")), Some(true));
        assert_eq!(r.matches(Some("2026-10-31T23:59:59-12:00")), Some(true));
        assert_eq!(r.matches(Some("2026-11-01T00:00:00-04:00")), Some(false));
        assert_eq!(r.matches(None), None);
        assert!(DateRange::parse(Some("2026-11-02"), Some("2026-11-01")).is_err());
        assert!(DateRange::parse(Some("2026-02-30"), None).is_err());
    }
    #[test]
    fn google_query_is_preserved_and_location_aliases_are_not_geocoded() {
        assert_eq!(
            compose_query(Some("Emma and Lucy October 2026"), None, &[]),
            "Emma and Lucy October 2026"
        );
        assert_eq!(
            compose_query(None, Some("Philly"), &["birthday cake".into()]),
            "Philly birthday cake"
        );
        assert_eq!(
            compose_query(None, Some("Philadelphia, Pennsylvania"), &[]),
            "Philadelphia, Pennsylvania"
        );
    }
}
