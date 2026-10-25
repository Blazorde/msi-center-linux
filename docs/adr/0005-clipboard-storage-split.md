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

## Mise à jour (phase 6)

- Épingler **déplace** la ligne (et ses tags) de la base de session vers la base persistante, dans une seule
  transaction : la session est ouverte comme base principale et `clips.db` y est attachée (`ATTACH`).
- Les deux bases utilisent le journal de rollback (`journal_mode=DELETE`) et **non WAL** : avec WAL, SQLite ne
  garantit pas l'atomicité d'une transaction qui touche plusieurs bases. Le WAL n'apportait rien ici (un seul
  processus). À l'ouverture, les clips de session dont le contenu existe déjà dans la base persistante sont
  supprimés (filet de sécurité).
- `secure_delete=ON` : un clip épinglé puis supprimé est effacé du fichier, pas laissé dans les pages libres.
- Limite de taille : 10 Mio (10 × 1024 × 1024 octets) pour tout clip, texte ou image.
- La purge au logout suppose que le *lingering* systemd est désactivé pour l'utilisateur (sinon logind conserve
  `$XDG_RUNTIME_DIR`).

## Mise à jour (2026-10-25) — taguer épingle

Décision du propriétaire : un clip tagué est un clip à garder. Règle métier dans `clip-core`
(`Library::set_tags`), pas dans `clipd` :

- `SetTags` avec un ensemble de tags normalisé **non vide** sur un clip non épinglé l'**épingle** (même chemin
  que `Pin` : déplacement vers `clips.db`), donc il survit au logout. Le clip est épinglé *avant* l'écriture des
  tags : un échec de stockage entre les deux laisse au pire un clip épinglé avec ses anciens tags, jamais un
  clip tagué effaçable au logout.
- Vider les tags **ne désépingle pas**.
- `Unpin` d'un clip tagué reste permis et **conserve les tags** (option la moins surprenante : désépingler ne
  doit pas détruire de métadonnées ; l'utilisateur peut toujours vider les tags). Le clip retourne en session
  et disparaît au logout ; le retaguer l'épingle à nouveau. Un clip non épinglé peut donc porter des tags.
- Signature D-Bus `SetTags (t id, as tags) → as tags` inchangée (rétrocompatible) ; `Library::set_tags`
  renvoie `SetTagsOutcome { tags, pinned }` et les clients D-Bus voient l'épinglage via `Changed` + `List`.
