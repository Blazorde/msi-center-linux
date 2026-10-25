# API D-Bus

> Les deux interfaces sont figées. Règle : changement incompatible = nouvelle interface (`…2`).

## `org.blazorde.MsiCenter1` — bus système, `msi-daemon`

> **Figée (phase 4).** Implémentation : `crates/msi-daemon/src/service.rs`, testée de bout en bout sur une
> connexion pair-à-pair avec un EC en mémoire (`crates/msi-daemon/tests/dbus.rs`, dont un test d'introspection).
> Client : `msictl` et le proxy `msi_daemon::client::MsiCenter1Proxy`.

Nom de bus : `org.blazorde.MsiCenter1` · Objet : `/org/blazorde/MsiCenter` · Interface : `org.blazorde.MsiCenter1`.
Activation D-Bus système (`systemd/org.blazorde.MsiCenter1.system.service`). Politique du bus :
`systemd/org.blazorde.MsiCenter1.conf` (seul root possède le nom, tout le monde peut appeler).

### Méthodes

| Méthode | Entrée | Sortie | polkit | Erreurs |
|---|---|---|---|---|
| `SetProfile` | `s profile` (`eco` · `balanced` · `gaming`) | – | `set-profile` | `InvalidArgument`, `ReadOnly`, `SafetyFallback`, `GpuBusy`, `Hardware` |
| `SetAutoProfileOnPower` | `b enabled` | – | `set-profile` | idem |
| `FanCurve` | `s fan` (`cpu` · `gpu`) | `(yyyyydayy) spec, (ayayay) table, a(yyyyyy) raised, b custom` | – | `InvalidArgument` |
| `SetFanCurve` | `s fan, (yyyyydayy) spec` | `(ayayay) table, a(yyyyyy) raised` | `set-fan-curve` | `InvalidArgument`, `ReadOnly`, `SafetyFallback` |
| `ResetFanCurve` | `s fan` | – | `set-fan-curve` | idem |
| `PreviewFanCurve` | `(yyyyydayy) spec` | `(ayayay) table, a(yyyyyy) raised` | – | `InvalidArgument` |
| `SetCoolerBoost` | `b enabled` | – | `set-fan-curve` | `InvalidArgument` (forcé par S4), `ReadOnly`, `SafetyFallback` |
| `SetChargeLimit` | `y limit` (50–100) | – | `set-battery` | `InvalidArgument`, `ReadOnly`, `Hardware` |
| `GpuHolders` | – | `a(us) holders` (pid, nom) | – | – |

Les actions polkit sont `org.blazorde.msicenter.<action>` (`systemd/org.blazorde.msicenter.policy`) : utilisateur
local actif autorisé sans mot de passe, sinon `auth_admin`. Refus → `NotAuthorized`. Toute méthode peut aussi
renvoyer `Internal`.

### Types

| Type | Signature | Champs |
|---|---|---|
| spec | `(yyyyydayy)` | `idle` %, `t0` °C, `t1` °C, `min` %, `max` %, `k`, `up` (6 seuils °C), `hysteresis` °C |
| table | `(ayayay)` | `speeds` (7 %), `up` (6 °C), `down` (6 °C) : la table écrite dans l'EC |
| raised | `a(yyyyyy)` | par palier relevé par S2 : `step`, `entry_temp` (°C, 0 pour le palier 0), `curve_speed`, `applied_speed`, `floor_temp`, `floor_speed` |

Bornes de `spec` (S3, rejet sans correction) : voir [fan-safety.md](../hardware/fan-safety.md#du-modèle-à-la-table-ec-modèle-par-paliers-mesuré).

### Signaux

| Signal | Signature | Sens |
|---|---|---|
| `Telemetry` | `a{sv} data` | ~1 Hz. Clés (absentes si inconnues) : `cpu_pct`, `ram_pct`, `igpu_pct`, `dgpu_pct`, `npu_pct`, `cpu_temp`, `gpu_temp`, `power_w` (`d`) ; `fan1_rpm`, `fan2_rpm` (`u`) ; `fan1_pct`, `fan2_pct` (`y`) ; `dgpu_state` (`s`) ; `cooler_boost` (`b`). Températures de l'EC (celles que suivent les ventilos), sinon hwmon/NVML. |
| `SafetyEvent` | `s kind, d value, s detail` | `kind` : `boost_forced` / `boost_released` (`value` = °C le plus chaud, 0 si capteur illisible), `readback_mismatch` / `write_failed` (EC remis en `auto`), `ec_reset` (tables perdues par l'EC, ré-appliquées). `detail` : texte anglais. |

`PropertiesChanged` (`org.freedesktop.DBus.Properties`) est émis pour `Profile`, `AutoProfileOnPower`,
`OnAcPower`, `FanMode`, `CoolerBoost`, `BoostForced`, `ChargeLimit`, `GpuState` quand ils changent (après un
appel ou à la boucle de 1 s).

### Propriétés (lecture seule)

| Propriété | Type | Sens |
|---|---|---|
| `Profile` | `s` | Profil choisi |
| `AutoProfileOnPower` | `b` | Profil automatique au branchement/débranchement secteur (secteur : `balanced`, batterie : `eco` par défaut) |
| `OnAcPower` | `b` | Sur secteur (`false` aussi si inconnu) |
| `FanMode` | `s` | Fan mode lu dans l'EC : `advanced` = nos courbes s'appliquent ; `auto` = courbe usine (démarrage, repli, lecture seule) ; `unknown` |
| `CoolerBoost` | `b` | Cooler Boost lu dans l'EC |
| `BoostForced` | `b` | Cooler Boost forcé par le superviseur thermique (S4) |
| `ChargeLimit` | `y` | Limite de charge (%), 0 si non supportée |
| `GpuState` | `s` | `active` · `suspended` · `removed` |
| `ReadOnly` | `b` | S8 : EC non supporté, aucune écriture |
| `ReadOnlyReason` | `s` | Pourquoi (vide sinon) |
| `FirmwareVersion` | `s` | Version firmware EC (vide si msi-ec absent) |

Les réglages modifiables passent par des **méthodes** (et non des propriétés en écriture) pour que chacun ait
son contrôle polkit et ses erreurs typées.

### Sémantique

- **`SetProfile`** : ordre d'application courbes (écritures ordonnées, une table complète par écriture) →
  `fan_mode=advanced` → `shift_mode` → relecture de tout (S7) → dGPU. Une divergence ou une écriture en échec
  remet l'EC en `auto`, émet `SafetyEvent` et renvoie `SafetyFallback`. Si l'étape dGPU est refusée (Éco : dGPU
  tenu par des processus, écran branché, scan incomplet) la partie EC reste appliquée, le profil reste choisi, et
  l'appel renvoie `GpuBusy` (liste avec `GpuHolders`). Aucun processus n'est jamais tué. Idempotent.
- **Courbes** : chaque profil a une courbe par ventilo, le preset du profil par défaut. `SetFanCurve` remplace
  celle du **profil choisi** pour ce ventilo (persistée), `ResetFanCurve` revient au preset. Les paliers relevés
  par les planchers S2 sont renvoyés dans `raised`, jamais cachés. `PreviewFanCurve` calcule sans rien écrire.
- **`SetCoolerBoost(false)`** pendant que S4 force le boost → `InvalidArgument`. Au relâchement, l'EC revient
  au choix du propriétaire.
- **Persistance** : profil, `AutoProfileOnPower`, limite de charge et courbes personnalisées dans
  `/var/lib/msi-center/state.toml`, ré-appliqués au démarrage, après la sortie de veille et quand l'EC a perdu
  les tables (vérifié toutes les 10 s).
- **Lecture seule (S8)** : firmware absent de la liste ou attributs de courbe absents → toutes les méthodes qui
  écrivent renvoient `ReadOnly`, l'EC reste en courbe usine.

### Erreurs

Nom `org.blazorde.MsiCenter1.Error.<X>`, message lisible en anglais :

| Nom | Cas |
|---|---|
| `NotAuthorized` | polkit a refusé (ou est injoignable). |
| `InvalidArgument` | Profil/ventilo inconnu, valeur hors bornes (S3), Cooler Boost forcé par S4. |
| `ReadOnly` | EC non supporté (S8). |
| `Hardware` | Écriture matérielle hors EC en échec (batterie, PCI). |
| `SafetyFallback` | Écriture ou relecture EC en échec : EC remis en fan mode `auto` (S7). |
| `GpuBusy` | Retrait du dGPU refusé ; la partie EC du profil est appliquée. |
| `Internal` | Échec interne (détail dans le journal). |

### Choix d'implémentation

`zbus` 6 sur **tokio** (`current_thread`). Le travail sysfs (bloquant) tourne sur le pool bloquant sous un mutex
unique partagé avec la boucle de supervision de 1 s ; les propriétés sont servies depuis un instantané
rafraîchi après chaque appel et chaque tick. polkit est interrogé via l'API D-Bus
`org.freedesktop.PolicyKit1.Authority.CheckAuthorization` (sujet `system-bus-name`), sans bibliothèque C.

## `org.blazorde.Clipd1` — bus session, `clipd`

> **Figée (phase 6).** Implémentation : `crates/clipd/src/service.rs`, testée de bout en bout
> (`crates/clipd/tests/dbus.rs`).

Nom de bus : `org.blazorde.Clipd1` · Objet : `/org/blazorde/Clipd` · Interface : `org.blazorde.Clipd1`.
Activation D-Bus (`systemd/org.blazorde.Clipd1.service`). Pas de polkit : bus session, même utilisateur.

### Méthodes

| Méthode | Entrée | Sortie | Erreurs |
|---|---|---|---|
| `Add` | `s mime, ay data` | `t id` | `Rejected` |
| `List` | `s query, as tags, s scope, u offset, u limit` | `u total, a(tsstxxbas) clips` | `InvalidArgument` |
| `Get` | `t id` | `s mime, ay data` | `NotFound` |
| `Pin` | `t id` | – | `NotFound` |
| `Unpin` | `t id` | – | `NotFound` |
| `Delete` | `at ids` | `u deleted` | – |
| `ClearUnpinned` | – | `u deleted` | – |
| `SetTags` | `t id, as tags` | `as tags` (normalisés) ; épingle si tags non vides | `NotFound`, `InvalidArgument` |
| `Tags` | – | `a(su) tags` (tag, nombre de clips) | – |

Toute méthode peut aussi renvoyer `Internal` (échec de stockage, détail dans le journal du service).

### Signal et propriétés

| Membre | Type | Signature | Sens |
|---|---|---|---|
| `Changed` | signal | `()` | Émis après toute modification (ajout, bump, pin, unpin, suppression, tags). Le client relit avec `List`. |
| `MaxUnpinned` | propriété (lecture) | `u` | Nombre max de clips non épinglés par session (500). |
| `MaxClipBytes` | propriété (lecture) | `t` | Taille max d'un clip en octets (10 Mio = 10 485 760). |

### Sémantique

- **`Add`** : `mime` est normalisé (minuscules, paramètres `; charset=…` retirés). Seuls `text/*` (UTF-8 valide)
  et `image/*` sont acceptés. Contenu vide ou > `MaxClipBytes` → `Rejected` (jamais tronqué). Un contenu
  identique (même MIME normalisé + mêmes octets, hash SHA-256) n'est pas dupliqué : le clip existant, épinglé
  ou non, remonte en tête (`last_used` mis à jour) et son id est renvoyé. Au-delà de `MaxUnpinned`, les clips
  non épinglés les moins récemment utilisés sont supprimés ; les épinglés ne le sont jamais.
- **`List`** : tri par dernière utilisation décroissante. `query` : sous-chaîne insensible à la casse du texte
  complet (vide = pas de filtre ; une image ne correspond jamais à un texte non vide). `tags` : le clip doit
  porter **tous** les tags (ET), normalisés comme pour `SetTags`. `scope` : `""` ou `all`, `pinned`,
  `unpinned`. `limit` : 0 = 50, plafonné à 1000. `total` = nombre de résultats avant pagination.
- **Entrée de `List`** `(tsstxxbas)` : `id`, `mime`, `preview` (une ligne, ≤ 200 caractères, `…` si coupé ;
  vide pour une image), `size` (octets), `created_ms`, `last_used_ms` (ms depuis l'epoch Unix), `pinned`,
  `tags` (triés).
- **`Pin` / `Unpin`** : idempotents. Épingler déplace le clip (et ses tags) vers la base persistante ;
  désépingler le renvoie (avec ses tags) dans l'historique de session, effacé au logout. L'id ne change pas.
  `SetTags` avec des tags épingle aussi (voir ci-dessous).
- **`Delete`** : épinglés ou non ; ids inconnus ignorés ; renvoie le nombre supprimé.
- **`SetTags`** : remplace les tags. Normalisation : trim, minuscules, espaces internes réduits à un, ≤ 32
  caractères, pas de caractère de contrôle, dédupliqués, triés, ≤ 16 par clip ; les entrées vides sont
  ignorées. Un tag invalide → `InvalidArgument`, rien n'est modifié.
  - **Taguer épingle** : si l'ensemble normalisé est non vide et que le clip n'est pas épinglé, il est
    épinglé (comme `Pin` : déplacé vers la base persistante, il survit au logout). Signature inchangée ;
    le client apprend l'épinglage via `Changed` puis `List` (champ `pinned`).
  - **Vider les tags ne désépingle pas** (`SetTags(id, [])` sur un clip épinglé le laisse épinglé).
  - **`Unpin` d'un clip tagué** reste permis et **conserve ses tags** : le clip retourne dans l'historique de
    session avec ses tags et disparaît au logout. Le retaguer l'épingle à nouveau.

### Erreurs

Nom `org.blazorde.Clipd1.Error.<X>`, message lisible en anglais :

| Nom | Cas |
|---|---|
| `NotFound` | Aucun clip avec cet id. |
| `Rejected` | Contenu refusé : vide, trop gros, MIME invalide ou non supporté, texte non UTF-8. |
| `InvalidArgument` | Tag invalide, `scope` inconnu. |
| `Internal` | Échec de stockage. |

### Choix d'implémentation

`zbus` 6 sur le runtime **tokio** (`current_thread`) : `spawn_blocking` exécute les appels SQLite (bloquants)
hors de la boucle async, `tokio::signal` gère SIGTERM/SIGINT pour un arrêt propre, et `zbus` s'intègre
nativement à tokio. Les appels sont sérialisés par un mutex : un seul utilisateur, quelques appels par seconde.
