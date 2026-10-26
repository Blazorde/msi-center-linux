//! D-Bus errors mapped to French messages for the user (pure).

/// Which service an error comes from (changes the "unavailable" wording).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// `msi-daemon` on the system bus.
    Daemon,
    /// `clipd` on the session bus.
    Clipd,
}

/// Error shown to the user. `detail` is the service's English message, kept for the journal and as a hint.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppError {
    /// The service is not running, not installed, or the bus is unreachable.
    #[error("{0:?} unavailable: {1}")]
    Unavailable(Service, String),
    /// polkit refused the action.
    #[error("not authorized")]
    NotAuthorized,
    /// A value was rejected (S3 bounds, unknown name, Cooler Boost forced by S4, invalid tag).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The EC is not supported (S8): read-only mode.
    #[error("read-only: {0}")]
    ReadOnly(String),
    /// A non-EC hardware write failed (battery, PCI).
    #[error("hardware: {0}")]
    Hardware(String),
    /// An EC write or read-back failed: the EC went back to its factory curve (S7).
    #[error("safety fallback: {0}")]
    SafetyFallback(String),
    /// The dGPU could not be removed; the EC part of the profile is applied.
    #[error("gpu busy: {0}")]
    GpuBusy(String),
    /// No clip with this id.
    #[error("not found")]
    NotFound,
    /// Clip content refused (empty, too big, unsupported type).
    #[error("rejected: {0}")]
    Rejected(String),
    /// Internal failure of the service, or anything else.
    #[error("internal: {0}")]
    Internal(String),
}

impl AppError {
    /// Maps a zbus error. Method errors are matched on the last component of their name, so both
    /// `org.blazorde.MsiCenter1.Error.X` and `org.blazorde.Clipd1.Error.X` work.
    #[must_use]
    pub fn from_zbus(e: &zbus::Error, service: Service) -> Self {
        match e {
            zbus::Error::MethodError(name, msg, _) => {
                Self::from_name(service, name.as_str(), msg.as_deref().unwrap_or(""))
            }
            zbus::Error::FDO(fdo) => {
                use zbus::DBusError as _;
                Self::from_name(service, fdo.name().as_str(), fdo.description().unwrap_or(""))
            }
            zbus::Error::InputOutput(_)
            | zbus::Error::Address(_)
            | zbus::Error::Handshake(_)
            | zbus::Error::InterfaceNotFound => Self::Unavailable(service, e.to_string()),
            other => Self::Internal(other.to_string()),
        }
    }

    /// Maps a D-Bus error name (full or short) and its message.
    #[must_use]
    pub fn from_name(service: Service, name: &str, msg: &str) -> Self {
        let detail = msg.to_owned();
        if name.starts_with("org.freedesktop.DBus.Error.") || name.starts_with("org.freedesktop.") {
            let short = name.rsplit('.').next().unwrap_or(name);
            return match short {
                "AccessDenied" | "AuthFailed" | "InteractiveAuthorizationRequired" => Self::NotAuthorized,
                "InvalidArgs" => Self::InvalidArgument(detail),
                "ServiceUnknown" | "NameHasNoOwner" | "NoServer" | "Disconnected" | "NoReply" | "Timeout"
                | "TimedOut" | "Spawn.ChildExited" | "UnknownObject" | "UnknownInterface"
                | "UnknownMethod" | "ChildExited" | "ExecFailed" | "ServiceNotFound" => {
                    Self::Unavailable(service, detail)
                }
                _ => Self::Internal(detail),
            };
        }
        match name.rsplit('.').next().unwrap_or(name) {
            "NotAuthorized" => Self::NotAuthorized,
            "InvalidArgument" => Self::InvalidArgument(detail),
            "ReadOnly" => Self::ReadOnly(detail),
            "Hardware" => Self::Hardware(detail),
            "SafetyFallback" => Self::SafetyFallback(detail),
            "GpuBusy" => Self::GpuBusy(detail),
            "NotFound" => Self::NotFound,
            "Rejected" => Self::Rejected(detail),
            _ => Self::Internal(detail),
        }
    }

    /// Whether the right reaction is the "service unavailable" empty state with a retry button.
    #[must_use]
    pub const fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable(..))
    }

    /// French message for a toast or a banner.
    #[must_use]
    pub fn message_fr(&self) -> String {
        match self {
            Self::Unavailable(Service::Daemon, _) => {
                "Le service msi-daemon est injoignable. Vérifiez qu'il est installé et démarré.".into()
            }
            Self::Unavailable(Service::Clipd, _) => {
                "Le service presse-papiers (clipd) est injoignable.".into()
            }
            Self::NotAuthorized => "Action refusée : autorisation système (polkit) non accordée.".into(),
            Self::InvalidArgument(d) => with_detail("Valeur refusée par les garde-fous", d),
            Self::ReadOnly(d) => with_detail(
                "Mode lecture seule : ce firmware EC n'est pas pris en charge, rien n'est écrit",
                d,
            ),
            Self::Hardware(d) => with_detail("Échec d'écriture matérielle", d),
            Self::SafetyFallback(d) => with_detail(
                "Écriture EC incohérente : par sécurité, l'EC est revenu à sa courbe d'usine",
                d,
            ),
            Self::GpuBusy(_) => "Profil appliqué, mais la carte graphique NVIDIA est utilisée : \
                                 elle reste allumée (voir Performance & GPU)."
                .into(),
            Self::NotFound => "Cet élément n'existe plus.".into(),
            Self::Rejected(d) => with_detail("Contenu refusé", d),
            Self::Internal(d) => with_detail("Erreur interne du service", d),
        }
    }
}

fn with_detail(base: &str, detail: &str) -> String {
    if detail.trim().is_empty() {
        format!("{base}.")
    } else {
        format!("{base} ({}).", detail.trim())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn method_error(name: &str, msg: &str) -> zbus::Error {
        zbus::Error::MethodError(
            zbus::names::OwnedErrorName::try_from(name.to_owned()).unwrap(),
            Some(msg.to_owned()),
            zbus::message::Message::method_call("/", "Ping")
                .build(&())
                .unwrap(),
        )
    }

    #[test]
    fn maps_daemon_errors() {
        let d = Service::Daemon;
        let cases = [
            (
                "org.blazorde.MsiCenter1.Error.NotAuthorized",
                AppError::NotAuthorized,
            ),
            (
                "org.blazorde.MsiCenter1.Error.InvalidArgument",
                AppError::InvalidArgument("x".into()),
            ),
            (
                "org.blazorde.MsiCenter1.Error.ReadOnly",
                AppError::ReadOnly("x".into()),
            ),
            (
                "org.blazorde.MsiCenter1.Error.Hardware",
                AppError::Hardware("x".into()),
            ),
            (
                "org.blazorde.MsiCenter1.Error.SafetyFallback",
                AppError::SafetyFallback("x".into()),
            ),
            (
                "org.blazorde.MsiCenter1.Error.GpuBusy",
                AppError::GpuBusy("x".into()),
            ),
            (
                "org.blazorde.MsiCenter1.Error.Internal",
                AppError::Internal("x".into()),
            ),
            (
                "org.freedesktop.DBus.Error.ServiceUnknown",
                AppError::Unavailable(d, "x".into()),
            ),
            ("org.freedesktop.DBus.Error.AccessDenied", AppError::NotAuthorized),
        ];
        for (name, want) in cases {
            assert_eq!(AppError::from_zbus(&method_error(name, "x"), d), want, "{name}");
        }
    }

    #[test]
    fn maps_clipd_errors() {
        let c = Service::Clipd;
        assert_eq!(
            AppError::from_zbus(&method_error("org.blazorde.Clipd1.Error.NotFound", "no"), c),
            AppError::NotFound
        );
        assert_eq!(
            AppError::from_zbus(&method_error("org.blazorde.Clipd1.Error.Rejected", "big"), c),
            AppError::Rejected("big".into())
        );
        assert!(AppError::from_name(c, "org.freedesktop.DBus.Error.NameHasNoOwner", "").is_unavailable());
    }

    #[test]
    fn io_errors_mean_unavailable() {
        let e = zbus::Error::InputOutput(std::sync::Arc::new(std::io::Error::other("gone")));
        assert!(AppError::from_zbus(&e, Service::Daemon).is_unavailable());
    }

    #[test]
    fn every_error_has_a_french_message() {
        let all = [
            AppError::Unavailable(Service::Daemon, String::new()),
            AppError::Unavailable(Service::Clipd, String::new()),
            AppError::NotAuthorized,
            AppError::InvalidArgument("t0 must be < t1".into()),
            AppError::ReadOnly(String::new()),
            AppError::Hardware("EIO".into()),
            AppError::SafetyFallback(String::new()),
            AppError::GpuBusy(String::new()),
            AppError::NotFound,
            AppError::Rejected(String::new()),
            AppError::Internal(String::new()),
        ];
        for e in all {
            let m = e.message_fr();
            assert!(m.ends_with('.'), "{m}");
            assert!(!m.contains("error"), "{m}");
        }
        assert_eq!(
            AppError::InvalidArgument("t0 must be < t1".into()).message_fr(),
            "Valeur refusée par les garde-fous (t0 must be < t1)."
        );
        assert_eq!(
            AppError::NotAuthorized.message_fr(),
            "Action refusée : autorisation système (polkit) non accordée."
        );
    }
}
