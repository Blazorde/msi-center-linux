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
| `SetProfile` | `s profile` (`eco` · `balanced` · `gaming`) | – | `set-profile` | `InvalidArgument`, `ReadOnly`, `SafetyFallback`, `GpuBusy` (écriture sysfs dGPU en échec), `Hardware` |
| `SetAutoProfileOnPower` | `b enabled` | – | `set-profile` | idem |
| `SetFanProfile` | `s fan_profile` (`silent` · `auto` · `balanced` · `turbo` · `custom`) | – | `set-fan-curve` | `InvalidArgument`, `ReadOnly`, `SafetyFallback`, `GpuBusy`, `Hardware` |
| `CustomFanCurve` | `s fan` (`cpu` · `gpu`) | `(yyyyydayy) spec, (ayayay) table, a(yyyyyy) raised` | – | `InvalidArgument` |
| `FanCurve` | `s fan` (`cpu` · `gpu`) | `(yyyyydayy) spec, (ayayay) table, a(yyyyyy) raised, b custom` | – | `InvalidArgument` |
| `SetFanCurve` | `s fan, (yyyyydayy) spec` | `(ayayay) table, a(yyyyyy) raised` | `set-fan-curve` | `InvalidArgument`, `ReadOnly`, `SafetyFallback` (passe le profil en `custom`, voir Sémantique) |
| `ResetFanCurve` | `s fan` | – | `set-fan-curve` | idem |
| `PreviewFanCurve` | `(yyyyydayy) spec` | `(ayayay) table, a(yyyyyy) raised` | – | `InvalidArgument` |
| `CpuStages` | – | `y idle, a(yyy) stages, a(yyy) floors, b saved, b active, b supported` | – | – |
| `SetCpuStages` | `y idle, a(yyy) stages` | `a(yyy) floors` | `set-fan-curve` | `InvalidArgument` (règle violée, ou firmware sans pilotage en direct), `ReadOnly`, `SafetyFallback` (passe le profil en `custom`, voir Sémantique) |
| `ResetCpuStages` | – | – | `set-fan-curve` | `ReadOnly`, `SafetyFallback` |
| `PreviewCpuStages` | `y idle, a(yyy) stages` | `a(yyy) floors` | – | `InvalidArgument` |
| `SetCoolerBoost` | `b enabled` | – | `set-fan-curve` | `InvalidArgument` (forcé par S4), `ReadOnly`, `SafetyFallback` |
| `RestoreFactory` | – | – | `set-fan-curve` | `ReadOnly` (S8), `SafetyFallback` (une étape a échoué ; `auto` tenté quand même) |
| `SetChargeLimit` | `y limit` (50–100) | – | `set-battery` | `InvalidArgument`, `ReadOnly`, `Hardware` |
| `GpuHolders` | – | `a(us) holders` (pid, nom) | – | – |

Les actions polkit sont `org.blazorde.msicenter.<action>` (`systemd/org.blazorde.msicenter.policy`) : utilisateur
local actif autorisé sans mot de passe, sinon `auth_admin`. Refus → `NotAuthorized`. Toute méthode peut aussi
renvoyer `Internal`.

### Types

| Type | Signature | Champs |
|---|---|---|
| spec | `(yyyyydayy)` | `idle` %, `t0` °C, `t1` °C, `min` %, `max` %, `k`, `up` (6 seuils °C), `hysteresis` °C |
| table | `(ayayay)` | `speeds` (7 %), `up` (6 °C), `down` (6 °C) : la table écrite dans l'EC. Pour `cpu` en pilotage hybride ([ADR-0010](../adr/0010-hybrid-cpu-fan-control.md)) : le **vecteur en vigueur**, seuils d'usine, paliers 0–3 à la consigne en direct (voir Sémantique) |
| stage | `(yyy)` | un palier de la courbe à paliers du CPU : `up` °C (montée), `down` °C (retombée), `speed` % |
| floor | `(yyy)` | plancher S2 qui prend le relais des paliers : `floor_temp` °C, `floor_speed` %, `curve_speed` % (ce que les paliers donnent à cette température en montée) |
| raised | `a(yyyyyy)` | par palier relevé par S2 : `step`, `entry_temp` (°C, 0 pour le palier 0), `curve_speed`, `applied_speed`, `floor_temp`, `floor_speed` |

Bornes de `spec` (S3, rejet sans correction) : voir [fan-safety.md](../hardware/fan-safety.md#du-modèle-à-la-table-ec-modèle-par-paliers-mesuré).

### Signaux

| Signal | Signature | Sens |
|---|---|---|
| `Telemetry` | `a{sv} data` | ~1 Hz. Clés (absentes si inconnues) : `cpu_pct`, `ram_pct`, `igpu_pct`, `dgpu_pct`, `npu_pct`, `cpu_temp`, `gpu_temp`, `power_w` (`d`) ; `fan1_rpm`, `fan2_rpm` (`u`) ; `fan1_pct`, `fan2_pct` (`y`) ; `cpu_fan_target_pct` (`y`, consigne en direct du ventilo CPU = vitesse des paliers CPU 0–3 en vigueur, présente seulement pendant le pilotage hybride, ADR-0010 ; ajoutée le 2026-10-26) ; `dgpu_state` (`s`) ; `cooler_boost` (`b`). Températures de l'EC (celles que suivent les ventilos), sinon hwmon/NVML. |
| `SafetyEvent` | `s kind, d value, s detail` | `kind` : `boost_forced` / `boost_released` (`value` = °C le plus chaud, 0 si capteur illisible), `readback_mismatch` / `write_failed` (EC remis sur ses tables d'usine en `auto`), `ec_reset` (tables perdues par l'EC, ré-appliquées), `fan_stalled` (S9, `value` = °C du capteur ; tables d'usine + `auto`, lecture seule jusqu'au redémarrage), `gpu_busy` (Éco : retrait du dGPU refusé, gardé en veille runtime ; `value` = 0, `detail` = raison avec les processus, ex. `dGPU is in use by nvidia-powerd (812), gnome-shell (3876); dGPU kept in runtime suspend`). `detail` : texte anglais. Les clients doivent ignorer un `kind` inconnu. |

`PropertiesChanged` (`org.freedesktop.DBus.Properties`) est émis pour `Profile`, `FanProfile`, `DefaultFanProfile`, `AutoProfileOnPower`,
`OnAcPower`, `FanMode`, `CoolerBoost`, `BoostForced`, `ChargeLimit`, `GpuState`, `ReadOnly`, `ReadOnlyReason`, `CpuPowerLimits` quand ils changent (après un
appel ou à la boucle de 1 s).

### Propriétés (lecture seule)

| Propriété | Type | Sens |
|---|---|---|
| `Profile` | `s` | Profil choisi |
| `FanProfile` | `s` | Profil de ventilation du profil choisi : `silent` · `auto` · `balanced` · `turbo` · `custom` (ajouté le 2026-10-26) |
| `DefaultFanProfile` | `s` | Profil de ventilation par défaut du profil choisi (`eco` → `silent`, `balanced` → `balanced`, `gaming` → `turbo`) |
| `AutoProfileOnPower` | `b` | Profil automatique au branchement/débranchement secteur (secteur : `balanced`, batterie : `eco` par défaut) |
| `OnAcPower` | `b` | Sur secteur (`false` aussi si inconnu) |
| `FanMode` | `s` | Fan mode lu dans l'EC : `advanced` = nos courbes s'appliquent ; `auto` = courbe usine (démarrage, repli, lecture seule) ; `unknown` |
| `CoolerBoost` | `b` | Cooler Boost lu dans l'EC |
| `BoostForced` | `b` | Cooler Boost forcé par le superviseur thermique (S4) |
| `ChargeLimit` | `y` | Limite de charge (%), 0 si non supportée |
| `GpuState` | `s` | `active` · `suspended` · `removed` |
| `ReadOnly` | `b` | Aucune écriture EC : S8 (EC non supporté) ou S9 (ventilo bloqué, jusqu'au redémarrage du daemon) |
| `ReadOnlyReason` | `s` | Pourquoi (vide sinon) ; commence par `S9 fan stall` pour S9 |
| `FirmwareVersion` | `s` | Version firmware EC (vide si msi-ec absent) |
| `CpuPowerLimits` | `(uu)` | Limites de puissance CPU en vigueur (PL1, PL2) en W, `(0, 0)` si illisibles ([ADR-0014](../adr/0014-cpu-power-limits.md)) |

Les réglages modifiables passent par des **méthodes** (et non des propriétés en écriture) pour que chacun ait
son contrôle polkit et ses erreurs typées.

### Sémantique

- **`SetProfile`** : ordre d'application courbes (écritures ordonnées, une table complète par écriture) →
  `fan_mode=advanced` → `shift_mode` → relecture de tout (S7) → dGPU. Une divergence ou une écriture en échec
  remet l'EC sur ses tables d'usine en `auto`, émet `SafetyEvent` et renvoie `SafetyFallback`. Si le retrait du
  dGPU est refusé (Éco : dGPU tenu par des processus, écran branché, scan incomplet) l'appel **réussit** : la
  partie EC est appliquée, `Profile` devient le profil demandé, le dGPU reste en veille runtime
  (`power/control=auto`) et `SafetyEvent("gpu_busy", 0, raison)` est émis (liste avec `GpuHolders`). Avant
  2026-10-26 l'appel renvoyait `GpuBusy` dans ce cas ; l'erreur n'est plus renvoyée que si une écriture sysfs
  du dGPU échoue (compatible : un client qui la traitait la verra simplement moins). Aucun processus n'est
  jamais tué. Idempotent.
- **Profils de ventilation** (ajoutés le 2026-10-26, compatibles : méthodes et propriétés nouvelles, signatures
  existantes inchangées ; [fan-safety.md](../hardware/fan-safety.md#profils-de-ventilation)) : chaque profil
  de performance a un profil de ventilation, par défaut `DefaultFanProfile` ; `SetFanProfile` choisit celui
  du **profil choisi**, le retient pour ce profil (persisté) et l'applique. `SetProfile` applique le profil de
  ventilation retenu du nouveau profil. Choisir le profil par défaut efface la préférence.
  - `silent` / `balanced` / `turbo` : presets Silencieux / Progressif / Agressif (GPU : table complète ; CPU :
    pilotage hybride avec cette courbe).
  - `custom` : la courbe personnalisée de chaque ventilo pour ce profil (`CustomFanCurve`) ; un ventilo jamais
    édité suit le preset du profil de ventilation par défaut.
  - `auto` : tables d'usine des deux ventilos (ordre sûr) puis `fan_mode=auto`, puis `shift_mode`, relecture
    (S7), dGPU. Pas de pilotage hybride, `cpu_fan_target_pct` absent, S9 non surveillé (courbe du firmware), S4
    actif. Contrairement à `RestoreFactory`, c'est un mode **choisi et persisté** : ré-appliqué au démarrage, à
    la sortie de veille et par le contrôle de dérive. `FanCurve` renvoie alors la table d'usine, `raised` vide,
    `custom` faux, et `spec` = preset du profil de ventilation par défaut (non appliqué). `FanMode` vaut `auto`.
- **`SetFanCurve`** = édition du profil **Personnalisé** : enregistre la courbe du ventilo pour le profil
  choisi, **passe ce profil en `custom`** et l'applique (même effet qu'avant pour un ancien client : la courbe
  personnalisée remplace le preset). Spec invalide → `InvalidArgument`, rien n'est écrit ni modifié.
  **`ResetFanCurve`** supprime la courbe de ce ventilo ; s'il n'en reste aucune et que le profil était en
  `custom`, il revient à son profil de ventilation par défaut. **`CustomFanCurve`** renvoie la courbe
  personnalisée (enregistrée, sinon le preset du profil de ventilation par défaut) avec sa table par paliers et
  ses relèvements S2 (comme `PreviewFanCurve`), qu'elle soit en vigueur ou non : c'est la source de l'éditeur.
- **`RestoreFactory`** : rend la main à l'EC : tables d'usine (ordre sûr, relues) puis `fan_mode=auto`
  ([fan-safety.md](../hardware/fan-safety.md#restitution-à-lec-tables-dusine)). Dure jusqu'au prochain
  `SetProfile`, `SetFanProfile`, `SetFanCurve`, `ResetFanCurve` ou `SetAutoProfileOnPower(true)` ; la sortie de veille, le
  contrôle de dérive et le profil automatique secteur n'y touchent pas. Non persisté : au redémarrage le daemon
  ré-applique le profil enregistré. Idempotent. `msictl factory`.
- **Arrêt** (S5) : le daemon, son hook de panic et `ExecStopPost` (`msi-daemon --restore-factory`, sans D-Bus)
  restituent les tables d'usine puis `auto`.
- **S9** : ventilo à 0 % sur puce ≥ 70 °C (filtrée) pendant 30 s → tables d'usine + `auto`, `fan_stalled`,
  `ReadOnly=true` jusqu'au redémarrage (`systemctl restart msi-daemon`).
- **Courbes** : la courbe d'un ventilo est celle du profil de ventilation du profil choisi (voir ci-dessus).
  Les paliers relevés par les planchers S2 sont renvoyés dans `raised`, jamais cachés. `PreviewFanCurve` calcule sans rien écrire.
- **Ventilo CPU piloté en direct** ([ADR-0010](../adr/0010-hybrid-cpu-fan-control.md), firmware dont les
  tables d'usine sont connues, `.110`) : l'EC ignore les seuils CPU écrits. Le daemon garde les seuils d'usine
  et réécrit `cpu/fan_curve_speeds` (≈ 1 Hz, seulement au changement) depuis la courbe `spec` évaluée sur la
  température CPU filtrée ([fan-safety.md](../hardware/fan-safety.md#pilotage-hybride-du-ventilo-cpu-adr-0010)).
  `FanCurve("cpu")` et `SetFanCurve("cpu", …)` renvoient le vecteur en vigueur (seuils d'usine, paliers 0–3 à la
  consigne, paliers chauds relevés et signalés dans `raised`) ; il change avec la température. Les `up` de
  `spec` ne s'appliquent qu'au GPU. `PreviewFanCurve` (sans ventilo) renvoie toujours la table par paliers de
  la courbe, celle du GPU. Le pilotage s'arrête dès que l'EC n'est plus sur nos tables (`RestoreFactory`,
  repli S7, S9, lecture seule). Compatible : signatures inchangées.
- **Courbe à paliers du CPU** ([ADR-0013](../adr/0013-staged-cpu-fan-curve.md), ajoutée le 2026-10-27,
  compatible : méthodes nouvelles, signatures existantes inchangées ;
  [fan-safety.md](../hardware/fan-safety.md#courbe-à-paliers-du-cpu-adr-0013)) : une vitesse de repos et de 1
  à **79** paliers `(up, down, speed)`. Le palier est atteint à `up` et quitté sous `down` (retombée propre à
  chaque palier). Règles (S1/S3, rejet sans correction → `InvalidArgument` qui nomme la règle) : `up` et `down`
  dans 20–100 °C, `up − down` ≥ 2 °C, `up` strictement croissants, `down` strictement croissants, vitesses
  ≤ 100 % et jamais décroissantes, repos ≤ vitesse du premier palier.
  - **`CpuStages`** : la courbe du profil choisi. `saved` faux : rien d'enregistré, la courbe renvoyée est le
    point de départ de l'éditeur (6 paliers tirés de la courbe personnalisée du CPU). `active` : la courbe
    pilote le ventilo CPU maintenant (enregistrée **et** profil de ventilation `custom`). `supported` faux :
    firmware sans tables d'usine connues, donc pas de pilotage en direct ; les paliers sont alors refusés.
    `floors` : voir type floor.
  - **`SetCpuStages`** : valide, enregistre pour le profil choisi, **passe ce profil en `custom`** et applique.
    Les planchers S2 qui prennent le relais sont renvoyés dans `floors` (jamais cachés) : une courbe plus lente
    qu'un plancher n'est pas rejetée, la consigne vaut `max(palier, plancher)`. **`PreviewCpuStages`** fait la
    même validation sans rien écrire. **`ResetCpuStages`** supprime la courbe (sans effet si aucune ; sans
    autre courbe personnalisée, le profil revient à son profil de ventilation par défaut).
  - **Dernière édition gagnante** : `SetFanCurve("cpu", …)` et `ResetFanCurve("cpu")` suppriment la courbe à
    paliers du profil choisi ; `SetCpuStages` remplace la courbe exponentielle du CPU. Le GPU garde sa courbe
    à 7 paliers.
  - Le vecteur écrit dans l'EC est celui de l'ADR-0010 (`FanCurve("cpu")` le renvoie, `custom` vrai) ; la
    consigne suit les paliers avec les rampes +5 %/s / −1 %/s.
- **`SetCoolerBoost(false)`** pendant que S4 force le boost → `InvalidArgument`. Au relâchement, l'EC revient
  au choix du propriétaire.
- **Persistance** : profil, `AutoProfileOnPower`, limite de charge, profil de ventilation par profil
  (`[fan_profiles]`) et courbes personnalisées dans `/var/lib/msi-center/state.toml` (version 3 : ajoute les
  courbes à paliers `[staged.<profil>]`, un fichier version 2 se charge tel quel ; un fichier
  version 1 est migré : un profil qui avait une courbe personnalisée passe en `custom`), ré-appliqués au démarrage, après la sortie de veille et quand l'EC a perdu
  les tables (vérifié toutes les 10 s).
- **Lecture seule** : S8 (firmware absent de la liste ou attributs de courbe absents) → toutes les méthodes qui
  écrivent renvoient `ReadOnly`, l'EC reste en courbe usine. Après S9 : idem jusqu'au redémarrage (sauf
  `RestoreFactory`, sans effet de plus) ; S4 continue de forcer Cooler Boost.

### Erreurs

Nom `org.blazorde.MsiCenter1.Error.<X>`, message lisible en anglais :

| Nom | Cas |
|---|---|
| `NotAuthorized` | polkit a refusé (ou est injoignable). |
| `InvalidArgument` | Profil/ventilo inconnu, valeur hors bornes (S3), Cooler Boost forcé par S4. |
| `ReadOnly` | EC non supporté (S8) ou arrêt de sécurité S9 jusqu'au redémarrage. |
| `Hardware` | Écriture matérielle hors EC en échec (batterie, PCI). |
| `SafetyFallback` | Écriture ou relecture EC en échec : EC remis sur ses tables d'usine en fan mode `auto` (S7). |
| `GpuBusy` | Écriture sysfs du dGPU en échec pendant un changement de profil ; la partie EC est appliquée. (Un retrait refusé n'est plus une erreur : `SafetyEvent("gpu_busy")`.) |
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
