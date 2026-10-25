# 0005 — Presse-papiers : tmpfs pour la session, SQLite pour les épinglés

- **Statut** : Accepté
- **Date** : 2026-10-25

## Contexte

Les clips non épinglés doivent disparaître à chaque logout ; les épinglés doivent durer. Tout est capturé, y
compris d'éventuels mots de passe (choix du propriétaire).

## Décision

- Non épinglés : `$XDG_RUNTIME_DIR/clipd/` (tmpfs). logind le supprime au logout → purge garantie même si `clipd`
  plante, et rien n'est jamais écrit sur le disque.
- Épinglés : `$XDG_DATA_HOME/clipd/clips.db` (SQLite, WAL, mode 0600). Épingler = copier du tmpfs vers la base.
- Limites : 500 éléments non épinglés (FIFO), 10 Mo par image, dédup par hash du contenu.

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Une seule base + purge au logout | Simple | Pas de purge si crash/coupure ; données sensibles sur disque |
| Tout en mémoire | Rapide | Perte au redémarrage de `clipd` |

## Conséquences

Deux backends de stockage derrière un même trait dans `clip-core`.
