# 0013 — Courbe à paliers du ventilo CPU (autant de paliers que souhaité)

- **Statut** : Proposé
- **Date** : 2026-10-27
- Complète [0006](0006-exponential-fan-curve.md), [0009](0009-step-curve-and-daemon-runtime.md),
  [0010](0010-hybrid-cpu-fan-control.md) et [0012](0012-fan-profiles.md).

## Contexte

Demande du propriétaire : des courbes « avec paliers ultra détaillés », avec **autant de paliers que voulu**,
chacun avec la puissance des ventilateurs, « jusqu'à une retombée en température ». Il a précisé : le CPU
peut avoir un nombre illimité de paliers, le GPU reste inchangé ; chaque palier a **sa propre** température de
montée, sa puissance et sa température de retombée.

Contrainte matérielle : l'EC n'a que **7 paliers par ventilateur** (6 seuils de montée, 6 de descente), et
aucun moyen de lui en faire stocker plus. Mais, d'après [0010](0010-hybrid-cpu-fan-control.md), l'EC **ignore
les seuils CPU écrits** et applique en < 2 s les **vitesses** CPU écrites en direct : le daemon pilote déjà le
ventilo CPU seconde par seconde. Un nombre quelconque de paliers n'est donc possible que pour le CPU, évalué
**par le daemon**, pas par l'EC. Le GPU, lui, suit ses seuils : il garde ses 7 paliers.

## Décision

1. `msi_core::staged` (pur, sans E/S) :
   - `StagedCurve` : une vitesse de **repos** (`idle`) et de 1 à **79** paliers `{up, down, speed}`. Le palier
     `i` est **atteint** à `up[i]` et **quitté** sous `down[i]` : `down[i] < up[i]` est la retombée propre à
     chaque palier (79 = un palier par °C de 22 à 100 °C, borne vérifiée par un test).
   - Validation (S1, S3, rejet sans correction) : `up` et `down` dans 20..=100 °C, écart `up − down` ≥ 2 °C,
     `up` strictement croissants, `down` strictement croissants, vitesses ≤ 100 % et **jamais décroissantes**,
     `idle` ≤ vitesse du premier palier.
   - `StageSelector` : quel palier est en vigueur (remonte à `up`, retombe sous `down`, hystérésis par
     palier ; température illisible → palier le plus chaud).
   - `StagedPilot` : consigne en direct, mêmes rampes que ADR-0010 (+5 %/s en montée, −1 %/s en descente, seuls
     sauts : arrêt et démarrage). Pas de bande morte : l'hystérésis de chaque palier joue ce rôle.
2. **S2 est un `max()` sur la consigne**, signalé : la consigne vaut `max(vitesse du palier, plancher S2 à la
   température)`. `StagedCurve::raised_floors()` liste les planchers au-dessus des paliers à leur propre
   température ; le daemon les renvoie (`SetCpuStages`, `PreviewCpuStages`), l'app les affiche. Rien n'est
   silencieux, et une courbe plus lente qu'un plancher n'est **pas rejetée** (comme les courbes exponentielles
   ADR-0006, contrairement à une table EC brute).
3. Le **vecteur écrit dans l'EC** reste celui de l'ADR-0010 : seuils d'usine, paliers 0–3 = consigne en
   direct, paliers 4–6 ≥ max(consigne, courbe à leur température d'entrée, 60 / 80 / 100 %). C'est
   `hybrid_cpu_table_for(&CpuCurve, …)`, où `CpuCurve` = `Exponential(FanCurveSpec) | Staged(StagedCurve)`
   (`hybrid_cpu_table` en est le cas exponentiel). `CpuDriver` garde la sortie quand on passe d'un type de
   courbe à l'autre : le ventilo ne retombe pas à une vitesse de départ pendant la bascule.
4. Le daemon (`controller`) :
   - stocke la courbe **par profil de performance** (`[staged.<profil>]` de `state.toml`, **version 3** ; la
     version 2 se charge telle quelle) ; une entrée invalide est ignorée avec un avertissement (S3 au
     chargement aussi) ;
   - ne l'applique que si le profil de ventilation est **`custom`** et qu'elle est enregistrée ; **dernière
     édition gagnante** : `SetFanCurve("cpu")` / `ResetFanCurve("cpu")` suppriment la courbe à paliers du
     profil, `SetCpuStages` passe le profil en `custom` ;
   - la **refuse** (`InvalidArgument`) sur un firmware sans tables d'usine connues (pas de pilotage en direct,
     donc rien ne peut suivre des paliers) et en lecture seule (`ReadOnly`) ;
   - garde S4, S5, S7, S9 inchangés : mêmes écritures vérifiées, mêmes replis.
5. API D-Bus **additive** (interface `…1` conservée) : `CpuStages`, `SetCpuStages`, `ResetCpuStages`,
   `PreviewCpuStages` ([dbus-api.md](../architecture/dbus-api.md)). polkit : `set-fan-curve` pour les deux
   méthodes qui modifient.
6. Clients : `msictl stages show|set|preview|reset` (`UP:DOWN:PUISSANCE`), éditeur GTK (liste de paliers à
   ajouter / insérer / supprimer, graphique en escalier avec la retombée en pointillés, planchers S2 signalés).
   Face à un ancien daemon, l'éditeur est masqué.

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Plus de 7 paliers dans l'EC | pas de logique côté daemon | impossible : 7 paliers en matériel |
| Paliers sans limite aussi pour le GPU | symétrique | le GPU suit ses seuils dans l'EC, pas de pilotage en direct : il faudrait le piloter en continu et perdre la sûreté « le daemon peut se figer » ; refusé par le propriétaire (« GPU inchangé ») |
| Retombée globale (une hystérésis pour tous) | plus simple à saisir | refusé par le propriétaire : seuil de retombée propre à chaque palier |
| Rejeter une courbe sous un plancher S2 | plus strict | incohérent avec ADR-0006/0012 et inutilement bloquant ; le relèvement signalé garde la même garantie |
| Interpoler les paliers en courbe continue | silhouette plus douce | ce n'est pas ce qui est demandé (paliers nets), et ajoute des paramètres |

## Conséquences

- Aucun changement incompatible. Ancienne app face à un nouveau daemon : ignore les méthodes ; nouvelle app
  face à un ancien daemon : éditeur masqué.
- **Sécurité** : le plafond reste l'EC. Si le daemon se fige, le vecteur déjà écrit garde les paliers chauds
  (≥ 60 % dès 77 °C lissés, 100 % dès 83 °C) et S5/S6 restituent les tables d'usine ; une courbe à paliers
  ne peut pas descendre sous S2 ni sous ces paliers chauds.
- Plusieurs dizaines de paliers donnent une courbe fine, mais la consigne est limitée par les rampes (+5 %/s,
  −1 %/s) : des paliers très rapprochés se fondent en une rampe. Le ventilo ne « saute » jamais d'un palier
  à l'autre.
- Le choix d'une courbe à paliers par profil de performance double la surface de `state.toml` (version 3) ;
  rétrograder le daemon sur une version 2 ignore simplement `[staged]`.
