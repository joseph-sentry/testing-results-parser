use std::fmt;
use std::ops::Range;

use timestamps::days_offset;
use watto::{align_to, Pod};

use super::*;

/// The current format version.
pub(crate) const TA_VERSION: u32 = 1;

/// The serialized [`TestAnalytics`] binary format.
///
/// This can be parsed from a binary buffer via [`TestAnalytics::parse`].
#[derive(Clone, PartialEq)]
pub struct TestAnalytics<'data> {
    header: &'data raw::Header,
    tests: &'data [raw::Test],
    timestamp: u32,

    total_pass_count: &'data [u16],
    total_fail_count: &'data [u16],
    total_skip_count: &'data [u16],
    total_flaky_fail_count: &'data [u16],
    total_duration: &'data [f32],

    last_timestamp: &'data [u32],
    last_duration: &'data [f32],

    string_bytes: &'data [u8],
}

impl<'data> TestAnalytics<'data> {
    /// Parses the given buffer into [`TestAnalytics`].
    pub fn parse(buf: &'data [u8], timestamp: u32) -> Result<Self, TestAnalyticsError> {
        let (header, rest) =
            raw::Header::ref_from_prefix(buf).ok_or(TestAnalyticsErrorKind::InvalidHeader)?;

        if header.magic != raw::TA_MAGIC {
            return Err(TestAnalyticsErrorKind::InvalidMagic(header.magic).into());
        }

        if header.version != TA_VERSION {
            return Err(TestAnalyticsErrorKind::WrongVersion(header.version).into());
        }

        let (_, rest) = align_to(rest, 8).ok_or(TestAnalyticsErrorKind::InvalidTables)?;
        let (tests, rest) = raw::Test::slice_from_prefix(rest, header.num_tests as usize)
            .ok_or(TestAnalyticsErrorKind::InvalidTables)?;

        let expected_data = header.num_tests as usize * header.num_days as usize;

        let (_, rest) = align_to(rest, 8).ok_or(TestAnalyticsErrorKind::InvalidTables)?;
        let (total_pass_count, rest) = u16::slice_from_prefix(rest, expected_data)
            .ok_or(TestAnalyticsErrorKind::InvalidTables)?;

        let (_, rest) = align_to(rest, 8).ok_or(TestAnalyticsErrorKind::InvalidTables)?;
        let (total_fail_count, rest) = u16::slice_from_prefix(rest, expected_data)
            .ok_or(TestAnalyticsErrorKind::InvalidTables)?;

        let (_, rest) = align_to(rest, 8).ok_or(TestAnalyticsErrorKind::InvalidTables)?;
        let (total_skip_count, rest) = u16::slice_from_prefix(rest, expected_data)
            .ok_or(TestAnalyticsErrorKind::InvalidTables)?;

        let (_, rest) = align_to(rest, 8).ok_or(TestAnalyticsErrorKind::InvalidTables)?;
        let (total_flaky_fail_count, rest) = u16::slice_from_prefix(rest, expected_data)
            .ok_or(TestAnalyticsErrorKind::InvalidTables)?;

        let (_, rest) = align_to(rest, 8).ok_or(TestAnalyticsErrorKind::InvalidTables)?;
        let (total_duration, rest) = f32::slice_from_prefix(rest, expected_data)
            .ok_or(TestAnalyticsErrorKind::InvalidTables)?;

        let (_, rest) = align_to(rest, 8).ok_or(TestAnalyticsErrorKind::InvalidTables)?;
        let (last_timestamp, rest) = u32::slice_from_prefix(rest, expected_data)
            .ok_or(TestAnalyticsErrorKind::InvalidTables)?;

        let (_, rest) = align_to(rest, 8).ok_or(TestAnalyticsErrorKind::InvalidTables)?;
        let (last_duration, rest) = f32::slice_from_prefix(rest, expected_data)
            .ok_or(TestAnalyticsErrorKind::InvalidTables)?;

        let (_, rest) = align_to(rest, 8).ok_or(TestAnalyticsErrorKind::UnexpectedStringBytes {
            expected: header.string_bytes as usize,
            found: 0,
        })?;
        let string_bytes = rest.get(..header.string_bytes as usize).ok_or(
            TestAnalyticsErrorKind::UnexpectedStringBytes {
                expected: header.string_bytes as usize,
                found: rest.len(),
            },
        )?;

        Ok(Self {
            header,
            tests,
            timestamp,

            total_pass_count,
            total_fail_count,
            total_skip_count,
            total_flaky_fail_count,
            total_duration,

            last_timestamp,
            last_duration,

            string_bytes,
        })
    }

    /// Iterates over the [`Test`]s included in the [`TestAnalytics`] summary.
    pub fn tests(&self) -> impl Iterator<Item = Result<Test<'data>, TestAnalyticsError>> + '_ {
        let num_days = self.header.num_days as usize;
        self.tests.iter().enumerate().map(move |(i, test)| {
            let start_idx = i * num_days;
            let mut end_idx = start_idx + num_days - 1;
            let latest_test_timestamp = self.last_timestamp[end_idx];

            // TODO: maybe move this offset logic someplace else
            let days_offset = days_offset(latest_test_timestamp, self.timestamp);
            let days_offset = if days_offset < 0 {
                // this means the stored data contains days/buckets in the *future*
                // in this case we just slice off the excess data
                end_idx = (end_idx as isize + days_offset) as usize;
                0
            } else {
                days_offset as usize
            };
            let data_range = start_idx..=end_idx;

            // TODO: maybe we want to resolve this on access, so we don’t have to do error handling here?
            let name = watto::StringTable::read(self.string_bytes, test.name_offset as usize)
                .map_err(|_| TestAnalyticsErrorKind::InvalidStringReference)?;

            Ok(Test {
                name,
                days_offset,
                total_pass_count: &self.total_pass_count[data_range.clone()],
                total_fail_count: &self.total_fail_count[data_range.clone()],
                total_skip_count: &self.total_skip_count[data_range.clone()],
                total_flaky_fail_count: &self.total_flaky_fail_count[data_range.clone()],
                total_duration: &self.total_duration[data_range.clone()],

                last_timestamp: &self.last_timestamp[data_range.clone()],
                last_duration: &self.last_duration[data_range.clone()],
            })
        })
    }
}

impl<'data> fmt::Debug for TestAnalytics<'data> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestAnalytics")
            .field("version", &self.header.version)
            .field("tests", &self.header.num_tests)
            .field("days", &self.header.num_days)
            .field("string_bytes", &self.header.string_bytes)
            .finish()
    }
}

/// This represents a specific test for which test analytics data is gathered.
#[derive(Clone, PartialEq)]
pub struct Test<'data> {
    name: &'data str,

    days_offset: usize,

    total_pass_count: &'data [u16],
    total_fail_count: &'data [u16],
    total_skip_count: &'data [u16],
    total_flaky_fail_count: &'data [u16],
    total_duration: &'data [f32],

    last_timestamp: &'data [u32],
    last_duration: &'data [f32],
}

impl<'data> Test<'data> {
    /// Returns the name of the test.
    pub fn name(&self) -> &'data str {
        self.name
    }

    /// Calculates aggregate data for the given [`Range`] of days.
    ///
    /// The day range should be given in reverse-notation, for example
    /// `7..0` to get the aggregates from 7 days ago till now.
    pub fn get_aggregates(&self, range: Range<usize>) -> Aggregates {
        // TODO: move this logic someplace else
        let num_days = self.total_pass_count.len();
        let range_start = (num_days + self.days_offset)
            .saturating_sub(range.start)
            .min(num_days);
        let range_end = (num_days + self.days_offset)
            .saturating_sub(range.end)
            .min(num_days);
        let range = range_start..range_end;

        let total_pass_count = self.total_pass_count[range.clone()]
            .iter()
            .map(|c| *c as u32)
            .sum();
        let total_fail_count = self.total_fail_count[range.clone()]
            .iter()
            .map(|c| *c as u32)
            .sum();
        let total_skip_count = self.total_skip_count[range.clone()]
            .iter()
            .map(|c| *c as u32)
            .sum();
        let total_flaky_fail_count = self.total_flaky_fail_count[range.clone()]
            .iter()
            .map(|c| *c as u32)
            .sum();
        let total_duration: f64 = self.total_duration[range.clone()]
            .iter()
            .map(|d| *d as f64)
            .sum();

        let total_run_count = total_pass_count + total_fail_count;
        let (failure_rate, flake_rate, avg_duration) = if total_run_count > 0 {
            (
                total_fail_count as f32 / total_run_count as f32,
                total_flaky_fail_count as f32 / total_run_count as f32,
                total_duration / total_run_count as f64,
            )
        } else {
            (0., 0., 0.)
        };

        Aggregates {
            total_pass_count,
            total_fail_count,
            total_skip_count,
            total_flaky_fail_count,

            failure_rate,
            flake_rate,

            avg_duration,
        }
    }
}

/// Contains test run data aggregated over a given time period.
#[derive(Clone, PartialEq)]
pub struct Aggregates {
    pub total_pass_count: u32,
    pub total_fail_count: u32,
    pub total_skip_count: u32,
    pub total_flaky_fail_count: u32,

    pub failure_rate: f32,
    pub flake_rate: f32,

    pub avg_duration: f64,
}
