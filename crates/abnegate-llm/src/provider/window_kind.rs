use std::fmt;

/// Which of a coding agent plan's usage windows a [`Window`] measures.
///
/// [`Self::streamed`] is the name claude's `rate_limit_event` gives the window
/// while a turn runs. An application that records windows under names of its
/// own, such as those its usage endpoint reports, maps each kind to them.
///
/// [`Window`]: crate::provider::Window
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WindowKind {
    /// The rolling five-hour session window.
    FiveHours,
    /// The rolling seven-day window across every model.
    SevenDays,
    /// The rolling seven-day window for Opus models.
    SevenDaysOpus,
    /// The rolling seven-day window for Sonnet models.
    SevenDaysSonnet,
}

impl WindowKind {
    /// Every kind, in the order a plan's windows are usually listed.
    pub const ALL: &[Self] = &[
        Self::FiveHours,
        Self::SevenDays,
        Self::SevenDaysOpus,
        Self::SevenDaysSonnet,
    ];

    /// This crate's stable name for the kind.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FiveHours => "five_hours",
            Self::SevenDays => "seven_days",
            Self::SevenDaysOpus => "seven_days_opus",
            Self::SevenDaysSonnet => "seven_days_sonnet",
        }
    }

    /// The `rateLimitType` claude's stream reports the window under.
    pub fn streamed(self) -> &'static str {
        match self {
            Self::FiveHours => "five_hour",
            Self::SevenDays => "seven_day",
            Self::SevenDaysOpus => "seven_day_opus",
            Self::SevenDaysSonnet => "seven_day_sonnet",
        }
    }

    /// The kind claude's stream reports as `name`, or `None` for a window,
    /// such as the overage one, that is not one of the plan's.
    pub fn from_streamed(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.streamed() == name)
    }
}

impl fmt::Display for WindowKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::WindowKind;

    #[test]
    fn every_kind_reads_back_from_the_name_claude_streams() {
        for (kind, streamed) in [
            (WindowKind::FiveHours, "five_hour"),
            (WindowKind::SevenDays, "seven_day"),
            (WindowKind::SevenDaysOpus, "seven_day_opus"),
            (WindowKind::SevenDaysSonnet, "seven_day_sonnet"),
        ] {
            assert_eq!(kind.streamed(), streamed);
            assert_eq!(WindowKind::from_streamed(streamed), Some(kind));
        }
        assert_eq!(WindowKind::ALL.len(), 4);
    }

    #[test]
    fn a_window_outside_the_plan_has_no_kind() {
        for name in ["seven_day_overage_included", "", "5h", "five_hours"] {
            assert_eq!(WindowKind::from_streamed(name), None, "{name}");
        }
    }

    #[test]
    fn every_kind_has_a_distinct_name_of_its_own() {
        let names: Vec<String> = WindowKind::ALL.iter().map(ToString::to_string).collect();
        assert_eq!(
            names,
            [
                "five_hours",
                "seven_days",
                "seven_days_opus",
                "seven_days_sonnet"
            ]
        );
    }
}
