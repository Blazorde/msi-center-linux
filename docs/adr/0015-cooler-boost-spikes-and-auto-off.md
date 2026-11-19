# 0015 — S4 ignore les rafales PL2, Cooler Boost se coupe tout seul

- **Statut** : Accepté
- **Date** : 2026-11-19

## Contexte

Retour du propriétaire (2026-11-19) : « faut que l'app ne prenne pas en compte les pics », « quand le cooler boost
est activé je suis obligé de le désactiver à la main », « j'ai l'impression que les ventilos ne suivent pas mes
courbes ».

Mesures sur la machine (journal `msi-daemon`, `msictl telemetry`, `msi-ec/cooler_boost`) :

- sous PL2 (60 W en Équilibré), le CPU monte à **95–98 °C pendant 2 à 10 s** plusieurs fois par heure. Le filtre
  de S4 (2 lectures ≥ 95 °C) forçait alors Cooler Boost à chaque rafale (`boost_forced` / `boost_released` en
  3–10 s) : ventilos à fond pour rien ;
- Cooler Boost est resté **allumé sans être forcé** après un relâchement S4 (10:12:33 → 10:16), ventilos à
  ~8 000 tr/min quelle que soit la courbe (l'EC affichait pourtant la consigne, 60 %). Coupé, le ventilo est
  revenu à la courbe (45 % ≈ 2 770 tr/min) et le CPU à 52 °C. Deux causes possibles, couvertes toutes les deux :
  le daemon prenait l'état de l'EC **au démarrage** comme choix du propriétaire (`user_boost`) et le remettait
  à chaque relâchement S4 ; et le firmware EC peut allumer Cooler Boost de lui-même (touche Fn, protection
  thermique), ce que rien ne coupait.

## Décision

1. **S4 regarde la chaleur soutenue, pas les rafales** (`msi_core::supervisor`) : Cooler Boost est forcé si les
   **10 dernières lectures** (~10 s, `FORCE_WINDOW`) sont toutes ≥ 95 °C, **ou** si les 2 dernières sont
   ≥ **100 °C** (`BOOST_CRITICAL_TEMP`, urgence). Relâchement inchangé (2 lectures < 85 °C), capteur illisible
   → forcé immédiatement (inchangé).
2. **Arrêt automatique de Cooler Boost** (`BoostAutoOff`) : un Cooler Boost **non forcé par S4** (app, touche
   Fn, firmware, trouvé allumé au démarrage) est coupé quand le CPU (température **filtrée**, ADR-0010) et le GPU
   restent sous **75 °C pendant 30 s**. Événement `boost_released` avec le détail `auto off: …`.
3. Le choix du propriétaire n'est plus lu dans l'EC au démarrage (`user_boost` = faux).

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Garder 2 lectures, monter le seuil à 100 °C | simple | une montée lente à 96–99 °C ne déclencherait jamais Cooler Boost |
| Moyenne glissante de 10 s ≥ 95 °C | lisse | une rafale à 110 °C + des lectures à 85 °C dépasse la moyenne ; minimum glissant plus lisible |
| Cooler Boost manuel jamais coupé | respecte le choix | c'est exactement la plainte du propriétaire |

## Conséquences

- Plus de Cooler Boost sur les rafales PL2 de quelques secondes ; une vraie surchauffe est prise en compte en
  ~10 s, une urgence (≥ 100 °C, le 185H throttle à 110 °C) en ~2 s. Les limites de puissance (ADR-0014) et les
  paliers chauds de l'EC (100 % dès 85 °C, S2) restent la première ligne.
- Cooler Boost devient un coup de pouce temporaire : allumé à la main, il s'éteint 30 s après que la machine a
  refroidi. Le propriétaire ne peut plus le laisser allumé en permanence à froid (demande explicite).
- Le daemon peut « lutter » toutes les 30 s contre un firmware qui rallume Cooler Boost à froid ; aucun cas
  observé.
