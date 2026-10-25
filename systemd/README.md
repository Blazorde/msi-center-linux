# systemd/

Phases 4, 6 et 9. Unités `msi-daemon.service` (système, `WatchdogSec`, `ExecStopPost` qui remet l'EC en `auto`),
`clipd.service` (utilisateur), fichiers de service D-Bus et politique polkit `org.blazorde.msicenter.policy`.
Voir [fan-safety.md](../docs/hardware/fan-safety.md) (invariants S5, S6).

| Fichier | Installé dans | Rôle |
|---|---|---|
| `clipd.service` | `/usr/lib/systemd/user/` | Service utilisateur `clipd` (`Type=dbus`), durci (`ProtectSystem=strict`, `PrivateNetwork`, filtre syscalls, `UMask=0077`). Arrêté avec la session graphique. |
| `org.blazorde.Clipd1.service` | `/usr/share/dbus-1/services/` | Activation D-Bus : le premier appel à `org.blazorde.Clipd1` démarre `clipd.service`. |

Vérification hors ligne (ne démarre rien) : `systemd-analyze --user security --offline=yes systemd/clipd.service`
(exposition 2.8, « OK »).
