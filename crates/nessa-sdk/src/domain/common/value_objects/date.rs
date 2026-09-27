use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateError;
impl fmt::Display for DateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "expected a valid calendar date in YYYY-MM or YYYY-MM-DD format"
        )
    }
}
impl Error for DateError {}

/// A calendar date with month or day precision. No time or timezone is implied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Date(String);
impl Date {
    pub fn new(value: String) -> Result<Self, DateError> {
        let complete = match value.len() {
            7 => format!("{value}-01"),
            10 => value.clone(),
            _ => return Err(DateError),
        };
        let parsed =
            chrono::NaiveDate::parse_from_str(&complete, "%Y-%m-%d").map_err(|_| DateError)?;
        // Chrono validates the calendar; this contract requires canonical spelling
        // and positive four-digit years. Month precision stays in the stored value.
        if chrono::Datelike::year(&parsed) < 1 || parsed.format("%Y-%m-%d").to_string() != complete
        {
            return Err(DateError);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn is_day(&self) -> bool {
        self.0.len() == 10
    }
}
