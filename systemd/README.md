# systemd/

Unités systemd, fichiers D-Bus et politique polkit. **Rien n'est installé ni démarré par le dépôt** : le
[PKGBUILD](../packaging/arch/PKGBUILD) les copie aux emplacements ci-dessous (sauf l'activation D-Bus système)
et ajoute les presets `80-msi-center-linux.preset` (`msi-daemon` et `clipd` activés à l'installation). Voir [fan-safety.md](../docs/hardware/fan-safety.md)
(invariants S5, S6) et [dbus-api.md](../docs/architecture/dbus-api.md).

| Fichier | Installé dans | Rôle |
|---|---|---|
| `msi-daemon.service` | `/usr/lib/systemd/system/` | Daemon système (`Type=notify`, `WatchdogSec=15`, `Restart=on-failure`). `ExecStopPost=msi-daemon --restore-factory` réécrit les tables d'usine puis remet l'EC en fan mode `auto` après tout arrêt (S5), y compris crash ou kill par le watchdog (S6). Réglages dans `/var/lib/msi-center/` (`StateDirectory`). |
| `org.blazorde.MsiCenter1.conf` | `/usr/share/dbus-1/system.d/` | Politique du bus système : seul root possède le nom ; tout le monde peut appeler (les méthodes qui modifient sont contrôlées par polkit dans le daemon). |
| `org.blazorde.MsiCenter1.system.service` | `/usr/share/dbus-1/system-services/org.blazorde.MsiCenter1.service` | Activation D-Bus système → `msi-daemon.service`. **Opt-in** : `scripts/dev-install.sh` ne l'installe qu'avec `--enable`, le paquet jamais (sinon un simple appel D-Bus démarrerait le daemon, qui réécrit l'EC). |
| `org.blazorde.msicenter.policy` | `/usr/share/polkit-1/actions/` | Actions `set-profile`, `set-fan-curve`, `set-battery` : utilisateur local actif autorisé sans mot de passe, sinon `auth_admin`. |
| `clipd.service` | `/usr/lib/systemd/user/` | Service utilisateur `clipd` (`Type=dbus`), durci (`ProtectSystem=strict`, `PrivateNetwork`, filtre syscalls, `UMask=0077`). Arrêté avec la session graphique. |
| `org.blazorde.Clipd1.service` | `/usr/share/dbus-1/services/` | Activation D-Bus : le premier appel à `org.blazorde.Clipd1` démarre `clipd.service`. |

## Durcissement de `msi-daemon.service`

Le daemon doit écrire dans `/sys` (attributs msi-ec, `charge_control_end_threshold`, `remove`/`rescan` PCI,
`power/control`) : pas de `ProtectKernelTunables`. `ProtectSystem=strict` laisse `/sys` inscriptible et ne
permet d'écrire que dans `/var/lib/msi-center`. Capacités limitées à `CAP_DAC_READ_SEARCH` + `CAP_SYS_PTRACE`
(lire `/proc/<pid>/fd` de tous les processus pour trouver les holders du dGPU) et `CAP_DAC_OVERRIDE`.
`DeviceAllow` n'ouvre que `/dev/nvidia*` (NVML en Gaming). À valider au premier lancement réel.

Vérification hors ligne (ne démarre rien) : `systemd-analyze security --offline=yes systemd/msi-daemon.service`
(exposition 1.9, « OK »), `systemd-analyze --user security --offline=yes systemd/clipd.service` (2.8, « OK »).
