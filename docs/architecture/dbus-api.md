# API D-Bus

> Brouillon. Les signatures seront figées en phase 4 (daemon) et 6 (clipd), avant les frontends.
> Règle : changement incompatible = nouvelle interface (`…2`).

## `org.blazorde.MsiCenter1` — bus système, `msi-daemon`

Objet : `/org/blazorde/MsiCenter`

| Membre | Type | Signature (prévue) | Polkit |
|---|---|---|---|
| `Profile` | propriété | `s` (`eco` · `balanced` · `gaming`) | lecture libre |
| `SetProfile` | méthode | `(s) → ()` | `org.blazorde.msicenter.set-profile` |
| `AutoProfileOnPower` | propriété rw | `b` | idem |
| `FanCurve` | propriété | `a{s(ddd)}` par ventilo : `min, max, exposant` | lecture libre |
| `SetFanCurve` | méthode | `(s fan, d t0, d t1, d min, d max, d k) → ()` | `…set-fan-curve` |
| `CoolerBoost` | propriété rw | `b` | `…set-fan-curve` |
| `ChargeLimit` | propriété rw | `y` (50–100) | `…set-battery` |
| `GpuState` | propriété | `s` (`active` · `suspended` · `removed`) | lecture libre |
| `GpuHolders` | méthode | `() → a(us)` (pid, nom) | lecture libre |
| `Telemetry` | signal | `a{sv}` (cpu_pct, dgpu_pct, igpu_pct, ram_pct, npu_pct, cpu_temp, gpu_temp, fan1_rpm, fan2_rpm, power_w) | – |
| `SafetyEvent` | signal | `(s kind, d value)` | – |

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
| `SetTags` | `t id, as tags` | `as tags` (normalisés) | `NotFound`, `InvalidArgument` |
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
  désépingler le renvoie dans l'historique de session, effacé au logout. L'id ne change pas.
- **`Delete`** : épinglés ou non ; ids inconnus ignorés ; renvoie le nombre supprimé.
- **`SetTags`** : remplace les tags. Normalisation : trim, minuscules, espaces internes réduits à un, ≤ 32
  caractères, pas de caractère de contrôle, dédupliqués, triés, ≤ 16 par clip ; les entrées vides sont
  ignorées. Les tags d'un clip non épinglé disparaissent avec lui au logout.

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
