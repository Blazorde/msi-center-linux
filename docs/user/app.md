# L'application MSI Center

`msi-center` est l'interface graphique (GTK 4 / libadwaita) de `msi-daemon` (ventilateurs, profils, carte
graphique, batterie) et de `clipd` (presse-papiers). Elle ne décide de rien : chaque réglage est validé et
appliqué par le service, avec les garde-fous de [sécurité ventilateurs](../hardware/fan-safety.md).

## Installer (utilisateur, sans sudo)

Le [paquet Arch](installation.md) installe l'app dans `/usr` avec son lancement automatique ; le script
ci-dessous sert au développement (ne pas mélanger les deux).

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
  Avec le paquet Arch, l'entrée système `/etc/xdg/autostart/org.blazorde.MsiCenter.desktop` lance l'app pour
  tous les utilisateurs : l'éteindre écrit alors une entrée utilisateur `Hidden=true` qui la masque.
  Si GNOME Tweaks l'a désactivé (`Hidden=true`), l'interrupteur apparaît éteint.
- **Démarrer en arrière-plan** : avec ou sans fenêtre à l'ouverture de session.
- **Thème** : Système, Clair ou Sombre (enregistré dans `~/.config/msi-center/settings.ini`).

## Changer de profil sans commande

Trois façons, toutes en un clic, sans terminal :

- **Accueil** : le grand sélecteur Éco / Équilibré / Gaming (icône, nom, description). Le profil cliqué
  s'éclaire tout de suite et affiche « Application… » le temps que le service l'applique ; en cas de refus
  (polkit, lecture seule), le sélecteur revient au profil réel et un message explique pourquoi. Si la
  RTX 4070 est encore utilisée en passant en Éco, un avertissement apparaît dans la carte avec
  **Voir les processus**.
- **Barre de titre** de toutes les autres pages : le même sélecteur en version compacte (icônes, infobulles),
  et le menu principal (section **Profil**). Raccourcis `Ctrl+1` (Éco), `Ctrl+2` (Équilibré), `Ctrl+3` (Gaming).
- **Clic droit sur l'icône** de MSI Center dans le dock ou le menu des applications : Éco, Équilibré, Gaming.
  Fonctionne aussi quand la fenêtre est fermée (notification en cas d'échec).

Pour l'extension et les scripts, l'app expose des actions GApplication :

```bash
gapplication action org.blazorde.MsiCenter profile-eco         # ou profile-balanced, profile-gaming
gapplication action org.blazorde.MsiCenter profile "'gaming'"  # action à paramètre (état = profil affiché)
```

## Les pages

| Page | Contenu |
|---|---|
| **Accueil** | Sélecteur de profil Éco / Équilibré / Gaming, état (alimentation, **profil de ventilation** — un clic ouvre Ventilateurs —, Cooler Boost, RTX), jauges en anneau animées : températures CPU/GPU, ventilateurs (tr/min et consigne), CPU, RTX 4070, iGPU, mémoire, NPU, puissance. |
| **Ventilateurs** | En tête, le **profil de ventilation** (voir ci-dessous). Puis, pour chaque ventilateur : la table à 7 paliers en graphique (marqueur de la température actuelle), la table écrite dans l'EC, les paliers **relevés par les planchers de sécurité S2** (signalés « ▲ »), l'éditeur de la courbe **Personnalisé** avec aperçu en direct, Appliquer, Annuler, Revenir au preset, Cooler Boost. |
| **Performance & GPU** | Ce que fait chaque profil, état de la RTX 4070, processus qui la gardent allumée, profil automatique secteur/batterie, firmware EC, mode lecture seule. |
| **Batterie** | Limite de charge 50–100 % (explication longévité), source d'alimentation. |
| **Presse-papiers** | Recherche, filtre par tags, liste ou grille, miniatures d'images, copier, épingler, tags, sélection multiple et suppression, effacement de l'historique non épinglé. |
| **Réglages** | Démarrage et thème. |

Raccourcis : `Ctrl+1` … `Ctrl+3` changent de profil, `Alt+1` … `Alt+6` changent de page, `Ctrl+F` cherche dans le presse-papiers, `Ctrl+W` ferme la
fenêtre, `Ctrl+Q` quitte.

### Profils de ventilation

Chaque profil (Éco, Équilibré, Gaming) a un **profil de ventilation**, choisi en un clic dans les cinq tuiles
de verre en haut de **Ventilateurs** (ou dans le menu principal, section Ventilation, ou par les actions
`app.fan::silent|auto|balanced|turbo|custom`) :

| Profil de ventilation | Ce qu'il fait | Par défaut pour |
|---|---|---|
| **Silencieux** | arrêté au repos, montée lente et tardive | Éco |
| **Auto** | **courbe d'usine MSI gérée par le contrôleur** (EC), comme sous Windows | – |
| **Équilibré** | démarrage doux, 100 % dès 85 °C | Équilibré |
| **Turbo** | monte tôt et fort, 100 % dès 74 °C, puis redescend | Gaming |
| **Personnalisé** | votre courbe (éditeur ci-dessous), planchers de sécurité inclus | – |

Le choix est **retenu par profil** : passer en Gaming ré-applique le profil de ventilation choisi pour Gaming.
La tuile du choix par défaut porte un badge « Défaut » ; **Revenir au défaut** l'y remet. Turbo n'est pas
Cooler Boost (ventilateurs à fond), qui reste un interrupteur séparé. En **Auto**, aucune courbe de MSI Center
n'est écrite (l'EC choisit ses paliers) ; la sécurité thermique Cooler Boost reste active.

### Courbe des ventilateurs (Personnalisé)

Les curseurs règlent la courbe exponentielle (vitesse au repos, T0, T1, vitesses de départ et maximale, k,
hystérésis). Bouger T0 ou T1 répartit les 6 seuils régulièrement entre les deux. L'aperçu (pointillés) est
calculé par le service **sans rien écrire** ; une valeur hors bornes (règle S3) est expliquée et bloque
**Appliquer**. Si la courbe est plus lente qu'un plancher de sécurité (30 % dès 60 °C, 50 % dès 75 °C,
100 % dès 85 °C), les paliers concernés sont relevés **et affichés** : rien n'est caché. L'éditeur part de
votre courbe personnalisée (sinon du preset par défaut du profil). **Appliquer** (« Appliquer en
Personnalisé » si un autre profil de ventilation est actif) enregistre la courbe de ce ventilateur pour le
profil actif et **passe ce profil en Personnalisé** ; **Revenir au preset** la supprime (sans courbe
personnalisée restante, le profil revient à son profil de ventilation par défaut).

### Courbe à paliers du CPU

Sous l'éditeur exponentiel, **Courbe à paliers (CPU)** permet autant de paliers que vous voulez (jusqu'à 79).
Chaque palier a trois réglages : la température où il **monte** (« Monte à ≥ »), la **puissance** du
ventilateur, et la température où il **retombe** (« Retombe sous ») vers le palier précédent. Entre les deux, le
ventilateur reste sur le palier : c'est la retombée en température propre à chaque palier. Une puissance au
repos s'applique sous le premier palier.

**Ajouter un palier** crée le suivant (5 °C plus haut, 10 points plus vite) ; le bouton « + » d'une ligne en
insère un entre deux paliers, la corbeille le supprime. Les valeurs hors règles (retombée au-dessus de la
montée, puissance qui diminue d'un palier à l'autre, moins de 2 °C entre retombée et montée…) sont expliquées
et bloquent **Appliquer**. Si vos paliers sont plus lents qu'un plancher de sécurité (30 % dès 60 °C, 50 % dès
75 °C, 100 % dès 85 °C), le plancher prend le relais et l'éditeur l'affiche. **Appliquer** enregistre la courbe
pour le profil actif et le passe en Personnalisé ; **Supprimer la courbe à paliers** revient à la courbe
exponentielle. Le GPU garde ses 7 paliers. En ligne de commande : `msictl stages set 20 55:50:30 65:60:45
75:70:70 85:80:100` (repos 20 %, puis `montée:retombée:puissance` par palier).

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
`#F28C38 → #C0362C → #2B2D42`, verre en couches (reflet spéculaire, bord intérieur, ombres de contact et de
profondeur), grands titres, chiffres tabulaires, jauges en anneau animées, variantes claire et sombre. Le
brief de design et les captures de chaque page sont dans [docs/design](../design/app-liquid-glass.md).

- **Mouvement réduit** : si les animations sont désactivées (Paramètres GNOME → Accessibilité, ou
  `gtk-enable-animations`), transitions et balayages des jauges sont supprimés.

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
MSI_CENTER_DEMO=1 cargo run -p msi-center    # mode démo : services simulés, aucun bus contacté
scripts/app-shots.sh                         # captures de toutes les pages, clair + sombre (GNOME headless isolé)
```

Le **mode démo** (`MSI_CENTER_DEMO=1`) sert `org.blazorde.MsiCenter1` et `org.blazorde.Clipd1` simulés dans
le processus, sur des connexions pair-à-pair privées (`src/demo/`) : télémétrie réaliste qui suit le profil,
courbes calculées par `msi-core`, historique de presse-papiers fictif, et un processus factice qui garde la
RTX 4070 quelques secondes en passant de Gaming à Éco. L'instance démo est non unique : elle ne prend
jamais la main sur une vraie instance de l'app. Seuls les services sont simulés : les réglages de l'app
(thème, démarrage) restent les vrais.

Le code testable (clients D-Bus, modèles de vue, géométrie des graphiques, autostart) est dans la
bibliothèque `crates/msi-center/src/` ; `tests/dbus.rs` le teste contre les vrais services en pair-à-pair,
sans bus ni écran. Les widgets sont dans `src/ui/`, le style dans `data/style.css` et `data/style-dark.css`.
