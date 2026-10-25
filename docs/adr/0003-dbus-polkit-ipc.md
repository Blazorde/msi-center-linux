# 0003 — IPC D-Bus (zbus) + polkit

- **Statut** : Accepté
- **Date** : 2026-10-25

## Contexte

Un daemon root doit être piloté par l'extension (GJS) et l'app GTK (Rust), sans exposer de pouvoir root arbitraire.

## Décision

`msi-daemon` expose `org.blazorde.MsiCenter1` sur le bus **système** et `clipd` expose `org.blazorde.Clipd1` sur le
bus **session**, via `zbus` (Rust pur, async). Les méthodes qui modifient le matériel exigent une action polkit
(`org.blazorde.msicenter.*`), autorisée sans mot de passe pour l'utilisateur actif local.

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Socket Unix + JSON | Simple | Auth, introspection et support GJS à réinventer |
| HTTP local | Universel | Surface d'attaque, inadapté au desktop |

## Conséquences

GJS a un support D-Bus natif (`Gio.DBusProxy`). Interfaces versionnées. Télémétrie par signaux (push), pas de polling.
