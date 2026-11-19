# 0016 — Bibliothèque de courbes : autant de courbes nommées que souhaité

- **Statut** : Accepté
- **Date** : 2026-11-19

## Contexte

Demande du propriétaire (2026-11-19) : « ajoute la possibilité de me faire des profils de courbes autant que je
veux ». Jusqu'ici, chaque profil de performance (Éco, Équilibré, Gaming) n'avait qu'**une** courbe
personnalisée par ventilo, plus une courbe à paliers CPU ; essayer une autre courbe effaçait la précédente.

## Décision

Une **bibliothèque** de courbes nommées dans le daemon (`state.toml` version 4, `[library."<nom>"]`) :

- une entrée = la photo des courbes **en vigueur** dans le profil choisi : courbe exponentielle CPU, courbe GPU,
  et la courbe à paliers CPU quand c'est elle qui pilote le CPU. Sous **Auto** (courbe d'usine), rien à
  enregistrer (`InvalidArgument`) ;
- **appliquer** copie l'entrée dans les courbes personnalisées du profil choisi (paliers compris, ou aucun) et
  le passe en **Personnalisé** ; chaque courbe est revalidée (S1–S3) avant toute écriture ;
- **supprimer** retire l'entrée sans toucher aux courbes en vigueur ;
- nom : 1 à 40 caractères une fois rogné, sans caractère de contrôle (`msi_core::library::CurveName`) ; un nom
  existant est remplacé. Aucune limite de nombre ;
- D-Bus (ajout compatible) : `SavedCurves`, `SaveCurves`, `ApplySavedCurves`, `DeleteSavedCurves`, polkit
  `set-fan-curve` pour les trois dernières. App : carte « Mes courbes » (Ventilateurs) ; extension : sous-menu
  « Mes courbes » ; `msictl curves [save|apply|delete] <nom>`.

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Profils de ventilation utilisateur à part entière (à côté de Silencieux, Turbo…) | une tuile par courbe | `FanProfile` est un enum stable (D-Bus, `GAction`, extension) ; changement de modèle lourd |
| Une courbe personnalisée de plus par profil | simple | toujours limité, pas « autant que je veux » |
| Bibliothèque côté app (fichier utilisateur) | pas de changement daemon | l'extension et `msictl` n'y ont pas accès ; validation dupliquée |

## Conséquences

- Les courbes enregistrées sont partagées par tous les profils de performance et par l'app, l'extension et
  `msictl`.
- Le fichier d'état passe en version 4 ; un daemon plus ancien ignore la table (ses courbes restent valides).
- Les entrées invalides au chargement sont abandonnées avec un avertissement (S3), comme les autres courbes.
