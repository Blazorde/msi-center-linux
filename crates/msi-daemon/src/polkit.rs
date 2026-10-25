//! polkit authorization of mutating D-Bus calls (ADR-0003), through the `org.freedesktop.PolicyKit1` D-Bus
//! API: no C library, no `unsafe`. Actions are declared in `systemd/org.blazorde.msicenter.policy`.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

use zbus::Variant;

/// polkit actions of the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Profile and AC auto-switch.
    SetProfile,
    /// Fan curves and Cooler Boost.
    SetFanCurve,
    /// Battery charge limit.
    SetBattery,
}

impl Action {
    /// All actions.
    pub const ALL: [Self; 3] = [Self::SetProfile, Self::SetFanCurve, Self::SetBattery];

    /// polkit action id.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::SetProfile => "org.blazorde.msicenter.set-profile",
            Self::SetFanCurve => "org.blazorde.msicenter.set-fan-curve",
            Self::SetBattery => "org.blazorde.msicenter.set-battery",
        }
    }
}

/// Result of an authorization check: `Ok(true)` allowed, `Ok(false)` denied, `Err` = polkit unreachable.
pub type AuthFuture<'a> = Pin<Box<dyn Future<Output = Result<bool, String>> + Send + 'a>>;

/// Decides whether the sender of a D-Bus call may perform an action. Injected so tests run without polkit.
pub trait Authorizer: fmt::Debug + Send + Sync {
    /// Checks `action` for the unique bus name `sender` (`None` on a peer-to-peer connection).
    fn check<'a>(&'a self, sender: Option<&'a str>, action: Action) -> AuthFuture<'a>;
}

#[zbus::proxy(
    interface = "org.freedesktop.PolicyKit1.Authority",
    default_service = "org.freedesktop.PolicyKit1",
    default_path = "/org/freedesktop/PolicyKit1/Authority"
)]
trait Authority {
    fn check_authorization(
        &self,
        subject: &(&str, HashMap<&str, Variant<'_>>),
        action_id: &str,
        details: &HashMap<&str, &str>,
        flags: u32,
        cancellation_id: &str,
    ) -> zbus::Result<(bool, bool, HashMap<String, String>)>;
}

/// `CheckAuthorization` flag: let polkit ask an authentication agent (for `auth_admin` fallbacks).
const ALLOW_USER_INTERACTION: u32 = 1;

/// The real authorizer, on the system bus.
#[derive(Debug, Clone)]
pub struct PolkitAuthorizer {
    conn: zbus::Connection,
}

impl PolkitAuthorizer {
    /// Authorizer using `conn` (the daemon's system bus connection).
    #[must_use]
    pub fn new(conn: zbus::Connection) -> Self {
        Self { conn }
    }
}

impl Authorizer for PolkitAuthorizer {
    fn check<'a>(&'a self, sender: Option<&'a str>, action: Action) -> AuthFuture<'a> {
        Box::pin(async move {
            // Without a sender (peer-to-peer) there is nobody to authorize.
            let Some(sender) = sender else {
                return Ok(false);
            };
            let proxy = AuthorityProxy::new(&self.conn)
                .await
                .map_err(|e| format!("polkit unavailable: {e}"))?;
            let subject = (
                "system-bus-name",
                HashMap::from([("name", Variant::from(sender))]),
            );
            let (authorized, _challenge, _details) = proxy
                .check_authorization(&subject, action.id(), &HashMap::new(), ALLOW_USER_INTERACTION, "")
                .await
                .map_err(|e| format!("polkit check failed: {e}"))?;
            Ok(authorized)
        })
    }
}

/// Allows exactly the listed actions, for everyone (tests, development on the session bus).
#[derive(Debug, Clone, Default)]
pub struct StaticAuthorizer {
    allowed: Vec<Action>,
}

impl StaticAuthorizer {
    /// Allows `allowed` only.
    #[must_use]
    pub fn new(allowed: &[Action]) -> Self {
        Self {
            allowed: allowed.to_vec(),
        }
    }

    /// Allows every action.
    #[must_use]
    pub fn allow_all() -> Self {
        Self::new(&Action::ALL)
    }
}

impl Authorizer for StaticAuthorizer {
    fn check<'a>(&'a self, _sender: Option<&'a str>, action: Action) -> AuthFuture<'a> {
        let allowed = self.allowed.contains(&action);
        Box::pin(async move { Ok(allowed) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids_match_the_policy_file() {
        let policy = include_str!("../../../systemd/org.blazorde.msicenter.policy");
        for a in Action::ALL {
            assert!(
                policy.contains(&format!("<action id=\"{}\">", a.id())),
                "{} missing from the policy",
                a.id()
            );
        }
    }
}
