# L'application MSI Center

`msi-center` est l'interface graphique (GTK 4 / libadwaita) de `msi-daemon` (ventilateurs, profils, carte
graphique, batterie) et de `clipd` (presse-papiers). Elle ne décide de rien : chaque réglage est validé et
appliqué par le service, avec les garde-fous de [sécurité ventilateurs](../hardware/fan-safety.md).

## Installer (utilisateur, sans sudo)

```bash
scripts/app-install.sh            # compile en release si besoin, puis installe
scripts/app-install.sh --uninstall
```

Le script installe dans `~/.local` :

| Fichier | Rôle |
|---|---|
| `~/.local/bin/msi-center` | le programme |
| `~/.local/share/applications/org.blazorde.MsiCenter.desktop` | entrée du menu des applications |
| `~/.local/share/icons/hicolor/…/org.blazorde.MsiCenter*.svg` | icônes (couleur et symbolique) |
| `~/.local/share/dbus-1/services/org.blazorde.MsiCenter.service` | ouverture par D-Bus (extension) |
| `~/.config/autostart/org.blazorde.MsiCenter.desktop` | lancement à chaque ouverture de session |

`msi-daemon` (système) et `clipd` (utilisateur) s'installent à part (voir [installation](installation.md)).
Sans eux, l'app reste utilisable : les pages concernées affichent « indisponible » avec un bouton
**Réessayer**, et retentent seules toutes les 10 secondes.

## Lancement automatique

L'app se lance **à chaque ouverture de session**, en arrière-plan (`msi-center --background`) : pas de
fenêtre, mais elle transforme les alertes de sécurité (surchauffe, Cooler Boost forcé, repli de l'EC) en
notifications. La fenêtre s'ouvre depuis le menu des applications, depuis l'extension (« Ouvrir MSI
Center ») ou en relançant `msi-center` (une seule instance tourne). Fermer la fenêtre ne quitte pas l'app
démarrée en arrière-plan ; **Menu → Quitter** (`Ctrl+Q`) l'arrête.

Dans **Réglages** :

- **Lancer à l'ouverture de session** : crée ou supprime `~/.config/autostart/org.blazorde.MsiCenter.desktop`.
  Si GNOME Tweaks l'a désactivé (`Hidden=true`), l'interrupteur apparaît éteint.
- **Démarrer en arrière-plan** : avec ou sans fenêtre à l'ouverture de session.
- **Thème** : Système, Clair ou Sombre (enregistré dans `~/.config/msi-center/settings.ini`).

## Les pages

| Page | Contenu |
|---|---|
| **Accueil** | Profil Éco / Balanced / Gaming, jauges en direct : températures CPU/GPU, ventilateurs (tr/min et consigne), CPU, RTX 4070, iGPU, mémoire, NPU, puissance. |
| **Ventilateurs** | Pour chaque ventilateur : la table à 7 paliers en graphique (marqueur de la température actuelle), la table écrite dans l'EC, les paliers **relevés par les planchers de sécurité S2** (signalés « ▲ »), le réglage de la courbe avec aperçu en direct, Appliquer, Annuler, Revenir au preset, Cooler Boost. |
| **Performance & GPU** | Ce que fait chaque profil, état de la RTX 4070, processus qui la gardent allumée, profil automatique secteur/batterie, firmware EC, mode lecture seule. |
| **Batterie** | Limite de charge 50–100 % (explication longévité), source d'alimentation. |
| **Presse-papiers** | Recherche, filtre par tags, liste ou grille, miniatures d'images, copier, épingler, tags, sélection multiple et suppression, effacement de l'historique non épinglé. |
| **Réglages** | Démarrage et thème. |

Raccourcis : `Alt+1` … `Alt+6` changent de page, `Ctrl+F` cherche dans le presse-papiers, `Ctrl+W` ferme la
fenêtre, `Ctrl+Q` quitte.

### Courbe des ventilateurs

Les curseurs règlent la courbe exponentielle (vitesse au repos, T0, T1, vitesses de départ et maximale, k,
hystérésis). Bouger T0 ou T1 répartit les 6 seuils régulièrement entre les deux. L'aperçu (pointillés) est
calculé par le service **sans rien écrire** ; une valeur hors bornes (règle S3) est expliquée et bloque
**Appliquer**. Si la courbe est plus lente qu'un plancher de sécurité (30 % dès 60 °C, 50 % dès 75 °C,
100 % dès 85 °C), les paliers concernés sont relevés **et affichés** : rien n'est caché. **Appliquer**
remplace la courbe du profil actif pour ce ventilateur ; **Revenir au preset** la supprime.

Le bouton **Rendre la main à l'EC** (courbes d'usine) n'apparaît que si le service fournit la méthode
`RestoreFactory` (l'app le détecte par introspection).

### Presse-papiers

**Ajouter un tag épingle l'élément** : il passe dans la base permanente et survit à la déconnexion. Vider
ses tags ne le désépingle pas ; le désépingler le renvoie dans l'historique de session, effacé au logout.

## Erreurs

Les erreurs du service sont traduites : autorisation refusée (polkit), valeur refusée par les garde-fous,
mode lecture seule, repli de sécurité de l'EC, carte graphique occupée… Elles apparaissent en bas de la
fenêtre ; les états durables (service injoignable, lecture seule, alerte de sécurité, RTX 4070 occupée) en
bandeau en haut.

## Style

Thème « Liquid Glass » aux couleurs du Golden Gate : orange international `#C0362C`, coucher de soleil
`#F28C38 → #C0362C → #2B2D42`, cartes translucides, reflets, ombres douces, variantes claire et sombre.

- GTK ne peut pas flouter le bureau **derrière** la fenêtre sous Wayland : le verre dépoli est simulé par un
  arrière-plan interne (dégradé, taches floutées, silhouette du pont, grain) vu à travers les cartes.
- En **contraste élevé** (Paramètres GNOME → Accessibilité), le style transparent est désactivé.
- Les informations ne passent jamais par la couleur seule : niveaux de température écrits (« normal »,
  « chaud », « critique »), paliers relevés marqués « ▲ », séries du graphique distinguées par le tracé
  (plein, tirets, pointillés, hachures) et une légende.
- L'app dessine sur l'iGPU Intel et n'ouvre jamais la RTX 4070 (sinon elle l'empêcherait de s'éteindre en
  Éco) ; voir [ADR-0011](../adr/0011-gtk-app-runtime-and-glass-style.md).

## Développement

```bash
cargo run -p msi-center                      # fenêtre
cargo run -p msi-center -- --background      # sans fenêtre
MSI_CENTER_SCREENSHOT=/tmp/a.png MSI_CENTER_PAGE=fans cargo run -p msi-center   # capture PNG puis quitte
```

Le code testable (clients D-Bus, modèles de vue, géométrie des graphiques, autostart) est dans la
bibliothèque `crates/msi-center/src/` ; `tests/dbus.rs` le teste contre les vrais services en pair-à-pair,
sans bus ni écran. Les widgets sont dans `src/ui/`, le style dans `data/style.css` et `data/style-dark.css`.
