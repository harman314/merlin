//! Bearer token for the local agent API.
//!
//! The file is the credential. Anything that can read it can read this
//! account's chats and send messages as the user, so it is created with
//! owner-only permissions and is never written to the log.

use std::path::Path;

use anyhow::{Context, Result};

/// Bytes of entropy behind the token.
const TOKEN_BYTES: usize = 32;

/// Hex-encoded secret shared with the agent.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    pub fn generate() -> Result<Self> {
        let mut bytes = [0u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes).context("Could not generate an agent token")?;
        Ok(Self(
            bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
        ))
    }

    /// Reads the token, creating one on first use.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if let Some(existing) = Self::load(path)? {
            return Ok(existing);
        }
        let token = Self::generate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Could not create {}", parent.display()))?;
        }
        std::fs::write(path, &token.0)
            .with_context(|| format!("Could not write {}", path.display()))?;
        restrict(path)?;
        Ok(token)
    }

    /// Reads the token without creating one.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let trimmed = contents.trim();
                if trimmed.is_empty() {
                    return Ok(None);
                }
                // A file left group- or world-readable is tightened in place
                // rather than silently trusted.
                restrict(path)?;
                Ok(Some(Self(trimmed.to_owned())))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => {
                Err(anyhow::Error::new(error).context(format!("Could not read {}", path.display())))
            }
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Compares without a length-dependent early exit on the shared prefix.
    ///
    /// The token's length is fixed and public, so differing lengths are
    /// rejected outright; only the secret bytes need the fixed-time walk.
    pub fn matches(&self, presented: &str) -> bool {
        let expected = self.0.as_bytes();
        let presented = presented.as_bytes();
        if expected.len() != presented.len() {
            return false;
        }
        let mut difference = 0u8;
        for (left, right) in expected.iter().zip(presented) {
            difference |= left ^ right;
        }
        difference == 0
    }

    /// Reads the token out of an `Authorization: Bearer` value.
    pub fn from_header(value: &str) -> Option<&str> {
        let (scheme, token) = value.split_once(' ')?;
        scheme
            .eq_ignore_ascii_case("bearer")
            .then(|| token.trim())
            .filter(|token| !token.is_empty())
    }
}

/// Keeps `Debug` from putting the credential in a log line.
impl std::fmt::Debug for Token {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Token(hidden)")
    }
}

#[cfg(unix)]
fn restrict(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("Could not restrict {}", path.display()))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> Result<()> {
    // Windows and macOS sandboxing aside, the per-user config directory is
    // already owner-only on Windows. Nothing further to set here.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_token_is_long_hex_and_never_repeats() {
        let first = Token::generate().expect("a token");
        let second = Token::generate().expect("a token");
        assert_eq!(first.as_str().len(), TOKEN_BYTES * 2);
        assert!(first.as_str().chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn the_token_never_prints_itself() {
        let token = Token::generate().expect("a token");
        let printed = format!("{token:?}");
        assert_eq!(printed, "Token(hidden)");
        assert!(!printed.contains(token.as_str()));
    }

    #[test]
    fn a_wrong_token_does_not_match_however_close_it_is() {
        let token = Token("abcdef".to_owned());
        assert!(token.matches("abcdef"));
        assert!(!token.matches("abcdeg"));
        assert!(!token.matches("abcde"));
        assert!(!token.matches("abcdefg"));
        assert!(!token.matches(""));
    }

    #[test]
    fn only_a_bearer_header_yields_a_token() {
        assert_eq!(Token::from_header("Bearer abc"), Some("abc"));
        assert_eq!(Token::from_header("bearer abc"), Some("abc"));
        assert_eq!(Token::from_header("Basic abc"), None);
        assert_eq!(Token::from_header("Bearer "), None);
        assert_eq!(Token::from_header("abc"), None);
    }

    #[test]
    fn the_first_read_creates_the_file_and_later_reads_return_it() {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("nested").join("agent-token");
        let created = Token::load_or_create(&path).expect("a token");
        let read = Token::load_or_create(&path).expect("the same token");
        assert_eq!(created, read);
        assert_eq!(
            std::fs::read_to_string(&path).expect("the file").trim(),
            created.as_str()
        );
    }

    /// The file is the credential, so a mode anyone could read is wrong.
    #[cfg(unix)]
    #[test]
    fn the_token_file_is_readable_by_its_owner_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("agent-token");
        Token::load_or_create(&path).expect("a token");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

        // A file loosened by hand is tightened on the next read.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        Token::load(&path).expect("a read");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_empty_file_is_treated_as_no_token_rather_than_an_empty_one() {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("agent-token");
        std::fs::write(&path, "  \n").expect("written");
        assert!(Token::load(&path).expect("a read").is_none());
        // Creating over it gives a usable token.
        let token = Token::load_or_create(&path).expect("a token");
        assert_eq!(token.as_str().len(), TOKEN_BYTES * 2);
    }

    #[test]
    fn a_missing_file_is_not_an_error_for_a_plain_read() {
        let dir = tempfile::tempdir().expect("a directory");
        assert!(
            Token::load(&dir.path().join("absent"))
                .expect("a read")
                .is_none()
        );
    }
}
