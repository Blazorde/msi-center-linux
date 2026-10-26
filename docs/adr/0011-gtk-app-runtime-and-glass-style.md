# 0011 — App GTK : D-Bus sur un thread tokio, style « Liquid Glass » sans sous-classe, rendu hors dGPU

- **Statut** : Proposé
- **Date** : 2026-10-26

## Contexte

`msi-center` (phase 8) est une app GTK 4 / libadwaita qui parle à `msi-daemon` (bus système) et à `clipd`
(bus session) via zbus. Contraintes :

- `zbus` est compilé avec sa fonctionnalité `tokio` pour tout le workspace (daemon et clipd tournent sur
  tokio) ; les fonctionnalités Cargo s'unifient, l'app ne peut pas choisir `async-io` seule sans toucher
  aux services.
- `unsafe` est interdit (`#![forbid(unsafe_code)]`). Les macros de sous-classe de gtk-rs
  (`#[glib::object_subclass]`, `glib::wrapper!`) génèrent de l'`unsafe` dans la crate : pas de widget
  sous-classé.
- Le propriétaire veut un style macOS « Liquid Glass » (verre dépoli) et une palette « Golden Gate ».
- GTK sur Wayland ne peut pas flouter ce qui est **derrière** la fenêtre (pas de `backdrop-filter`, le
  compositeur ne donne pas les pixels du bureau).
- Le renderer GTK par défaut énumère tous les GPU Vulkan/EGL au démarrage, ce qui ouvre `/dev/nvidia*` :
  l'app apparaissait dans `GpuHolders`, réveillait la RTX 4070 et empêchait le profil Éco de la retirer.

## Décision

1. **D-Bus sur un runtime tokio à un seul worker**, ponté vers GTK : l'UI appelle
   `Backend::daemon(|d| async move { … })`, qui lance la tâche sur tokio et renvoie le `JoinHandle`, attendu
   depuis `glib::spawn_future_local` (un `JoinHandle` est un futur ordinaire, exécutable par glib). Les
   événements poussés (télémétrie, `SafetyEvent`, `PropertiesChanged`, `Changed`) arrivent par un canal
   `tokio::sync::mpsc` non borné dont le récepteur est indépendant de l'exécuteur. Aucun widget ne quitte le
   thread GTK. Un superviseur par service reconnecte (perte du nom → « indisponible » → nouvelle tentative
   sur demande ou toutes les 10 s).
2. **Toute la logique testable dans la bibliothèque** `msi_center` (clients D-Bus typés, modèles de vue,
   messages d'erreur en français, géométrie des graphiques, autostart, préférences) ; le binaire ne contient
   que les widgets. Les clients sont testés sans bus ni écran contre les vraies interfaces de `msi-daemon` et
   `clipd` en pair-à-pair.
3. **Verre dépoli simulé à l'intérieur de la fenêtre** : un arrière-plan (dégradé coucher de soleil, taches
   de couleur floutées par `gtk::Snapshot::push_blur`, silhouette du Golden Gate, grain) est construit une
   fois dans un `gtk::Snapshot`, rendu en texture par le renderer de la fenêtre et affiché dans un
   `gtk::Picture` sous le contenu ; les cartes translucides (CSS) le laissent voir. Pas de coût par image,
   pas de sous-classe. Graphiques et jauges : `gtk::DrawingArea` + cairo. CSS clair + surcouche sombre
   suivant `AdwStyleManager:dark` ; en contraste élevé, le style transparent est retiré.
4. **Rendu hors dGPU** : au démarrage, le binaire se ré-exécute une fois avec
   `VK_LOADER_DRIVERS_DISABLE=*nvidia*` et `__EGL_VENDOR_LIBRARY_FILENAMES=…/50_mesa.json` (si absents),
   le rendu reste sur l'iGPU Intel. Ré-exécution plutôt que `set_var`, qui est `unsafe` en Rust 2024.

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| zbus `async-io` sur le contexte glib | Pas de thread en plus | Unification des features : change aussi le daemon et clipd |
| Proxies bloquants dans des threads | Simple | Un thread par appel, signaux difficiles à relayer |
| Widgets sous-classés (`glib::object_subclass`) | API GTK idiomatique, `push_blur` par image | Génère de l'`unsafe` : exigerait un ADR d'exception |
| Fenêtre transparente + flou du compositeur | Vrai verre | Impossible sous GNOME Wayland (pas de protocole de flou) |
| `GSK_RENDERER=cairo` | Aucun GPU | Rendu logiciel, flou et ombres lents |

## Conséquences

- Un thread de plus (≈ quelques Mo), aucune dépendance nouvelle hors gtk4-rs / libadwaita-rs (MIT).
- L'arrière-plan est une texture fixe (redessinée au changement clair/sombre) : il ne réagit pas au
  redimensionnement autrement que par mise à l'échelle (`ContentFit::Cover`), suffisant pour un flou.
- Toute nouvelle variable d'environnement GPU passe par `msi_center::gpu_env` (testé).
