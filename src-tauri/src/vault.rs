//! Provider credentials, in the OS keychain.
//!
//! macOS Keychain Services and Windows Credential Manager, via `keyring`.
//!
//! The one rule that matters: **a secret never crosses into the webview.** The
//! UI can ask whether a key is set and gets a bool; only this process reads the
//! value, and only at request-assembly time.

use hickeyfield_core::ProviderId;

const SERVICE: &str = "ai.hickeyfield.keys";

/// Suffix for the second half of a key/secret pair (Higgsfield only).
const SECRET_SUFFIX: &str = ".secret";

fn account(provider: ProviderId, secret_half: bool) -> String {
    if secret_half {
        format!("{}{SECRET_SUFFIX}", provider.slug())
    } else {
        provider.slug().to_string()
    }
}

/// Dev-only override, e.g. `hickeyfield_FAL_KEY`. Checked before the keychain so a
/// contributor can run without touching their login keychain at all.
fn env_override(provider: ProviderId, secret_half: bool) -> Option<String> {
    let var = format!(
        "hickeyfield_{}_{}",
        provider.slug().to_uppercase().replace('-', "_"),
        if secret_half { "SECRET" } else { "KEY" }
    );
    std::env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn entry(provider: ProviderId, secret_half: bool) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, &account(provider, secret_half)).map_err(|e| e.to_string())
}

/// Read a credential. Returns `None` rather than an error when absent — a
/// missing key is an ordinary state, not a failure.
pub fn get(provider: ProviderId, secret_half: bool) -> Option<String> {
    if let Some(v) = env_override(provider, secret_half) {
        return Some(v);
    }
    entry(provider, secret_half)
        .ok()?
        .get_password()
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

/// Write a credential. An empty value clears it.
///
/// Deletes before writing. A keychain item created by a *different* process —
/// the `security` CLI, an earlier unsigned build — carries an ACL that excludes
/// this app, and `get_password` then fails opaquely. Deleting first forces the
/// item to be recreated under our own ACL.
pub fn set(provider: ProviderId, secret_half: bool, value: &str) -> Result<(), String> {
    let entry = entry(provider, secret_half)?;
    let value = value.trim();
    let _ = entry.delete_credential();
    if !value.is_empty() {
        entry.set_password(value).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// What the UI is allowed to know about a provider's credentials.
#[derive(Debug, Clone, serde::Serialize)]
pub struct KeyState {
    pub provider: String,
    pub has_key: bool,
    /// Only meaningful for providers that use a key/secret pair.
    pub has_secret: bool,
    pub needs_key: bool,
    pub needs_secret: bool,
}

pub fn state(provider: ProviderId) -> KeyState {
    KeyState {
        provider: provider.slug().to_string(),
        has_key: get(provider, false).is_some(),
        has_secret: get(provider, true).is_some(),
        needs_key: provider.needs_key(),
        needs_secret: provider.needs_secret(),
    }
}

/// A provider is usable when everything it needs is present. `Local` needs
/// nothing, so it is always "configured" — whether it is actually *reachable*
/// is a separate detection concern.
pub fn is_configured(provider: ProviderId) -> bool {
    let s = state(provider);
    (!s.needs_key || s.has_key) && (!s.needs_secret || s.has_secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_names_are_stable_and_distinct() {
        assert_eq!(account(ProviderId::Fal, false), "fal");
        assert_eq!(account(ProviderId::Higgsfield, true), "higgsfield.secret");
        assert_ne!(
            account(ProviderId::Higgsfield, false),
            account(ProviderId::Higgsfield, true)
        );
    }

    #[test]
    fn local_is_configured_without_any_credential() {
        assert!(is_configured(ProviderId::Local));
    }

    #[test]
    fn env_override_is_read_and_trimmed() {
        // Scoped to a provider unlikely to be set in a real environment.
        std::env::set_var("hickeyfield_RECRAFT_KEY", "  abc123  ");
        assert_eq!(get(ProviderId::Recraft, false).as_deref(), Some("abc123"));
        std::env::remove_var("hickeyfield_RECRAFT_KEY");
    }

    #[test]
    fn blank_env_override_is_ignored() {
        std::env::set_var("hickeyfield_BFL_KEY", "   ");
        assert_eq!(env_override(ProviderId::Bfl, false), None);
        std::env::remove_var("hickeyfield_BFL_KEY");
    }
}
