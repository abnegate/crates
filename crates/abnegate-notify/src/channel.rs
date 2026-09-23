//! The name a delivery is reported under.

use std::borrow::Cow;
use std::fmt;

/// The kind of destination a [`Notifier`](crate::Notifier) delivers to.
///
/// Open rather than a closed enum so a channel can live outside this crate:
/// an application's own workspace chat should be able to join a fan-out
/// without this crate having heard of it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Channel(Cow<'static, str>);

impl Channel {
    pub const EMAIL: Self = Self(Cow::Borrowed("email"));
    pub const SLACK: Self = Self(Cow::Borrowed("slack"));
    pub const DISCORD: Self = Self(Cow::Borrowed("discord"));

    pub fn custom(name: impl Into<Cow<'static, str>>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for Channel {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_channels_name_themselves() {
        assert_eq!(Channel::EMAIL.as_str(), "email");
        assert_eq!(Channel::SLACK.to_string(), "slack");
        assert_eq!(Channel::DISCORD.as_str(), "discord");
    }

    #[test]
    fn a_custom_channel_carries_its_own_name() {
        let channel = Channel::custom("workspace-chat");
        assert_eq!(channel.as_str(), "workspace-chat");
        assert_ne!(channel, Channel::SLACK);
    }
}
