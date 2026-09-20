//! Whether this process is the installed app or a build from the source tree.
//!
//! Both are the same program, so without a switch the second launch finds the
//! first on the instance port and only raises its window. `MERLIN_PROFILE=dev`
//! gives this process its own directories, instance port and name, and with
//! them its own linked WhatsApp device, so the two run side by side.
//! `scripts/dev-run.sh` sets it.

use std::sync::LazyLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    Installed,
    Dev,
}

static CURRENT: LazyLock<Profile> =
    LazyLock::new(|| Profile::from_env(std::env::var("MERLIN_PROFILE").ok().as_deref()));

impl Profile {
    /// This process's profile, read once from the environment.
    pub fn current() -> Self {
        *CURRENT
    }

    fn from_env(value: Option<&str>) -> Self {
        match value {
            Some("dev") => Self::Dev,
            _ => Self::Installed,
        }
    }

    /// Directory name under the platform's application folders.
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Installed => "merlin",
            Self::Dev => "merlin-dev",
        }
    }

    /// Name shown in the window, the dock and the menu bar.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Installed => "Merlin",
            Self::Dev => "Merlin Dev",
        }
    }

    /// Loopback port the running instance claims. Outside the ephemeral range,
    /// and distinct from the one ZapFast and FastsApp share.
    pub fn instance_port(self) -> u16 {
        match self {
            Self::Installed => 47_143,
            Self::Dev => 47_144,
        }
    }

    /// Wire identity, so neither profile answers the other's handshake.
    pub fn wire_prefix(self) -> &'static str {
        match self {
            Self::Installed => "merlin:",
            Self::Dev => "merlin-dev:",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_dev_value_selects_the_separate_app() {
        assert_eq!(Profile::from_env(Some("dev")), Profile::Dev);
        assert_eq!(Profile::from_env(Some("")), Profile::Installed);
        assert_eq!(Profile::from_env(Some("release")), Profile::Installed);
        assert_eq!(Profile::from_env(None), Profile::Installed);
    }

    /// Sharing any of these would make one profile take over the other's
    /// archive, window or device link.
    #[test]
    fn the_two_profiles_share_nothing_that_would_collide() {
        let (installed, dev) = (Profile::Installed, Profile::Dev);
        assert_ne!(installed.dir_name(), dev.dir_name());
        assert_ne!(installed.display_name(), dev.display_name());
        assert_ne!(installed.instance_port(), dev.instance_port());
        assert_ne!(installed.wire_prefix(), dev.wire_prefix());
    }

    /// The installed profile keeps the names existing installations already
    /// use, so an upgrade finds its archive where it left it.
    #[test]
    fn the_installed_profile_keeps_its_original_names() {
        assert_eq!(Profile::Installed.dir_name(), "merlin");
        assert_eq!(Profile::Installed.display_name(), "Merlin");
        assert_eq!(Profile::Installed.instance_port(), 47_143);
        assert_eq!(Profile::Installed.wire_prefix(), "merlin:");
    }
}
