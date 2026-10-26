# Installation

Deux paquets Arch (CachyOS), construits depuis le dépôt par [`scripts/package.sh`](../../scripts/package.sh)
(recette : [`packaging/arch/PKGBUILD`](../../packaging/arch/PKGBUILD)) :

| Paquet | Contenu |
|---|---|
| `msi-center-linux` | `msi-daemon`, `msictl`, `clipd`, l'app `msi-center`, unités systemd et presets, fichiers D-Bus et polkit, entrées de menu et d'autostart, icônes, extension GNOME (schéma compilé) |
| `msi-ec-blazorde-dkms` | sources du module `msi-ec` modifié dans `/usr/src/msi-ec-blazorde-0.13.1/` (recompilé par DKMS pour chaque noyau), `/usr/lib/modules-load.d/msi-ec.conf` |

`msi-center-linux` dépend de `msi-ec-blazorde-dkms` ; ce dernier est en conflit avec les paquets DKMS upstream
(`msi-ec`, `msi-ec-dkms`, `msi-ec-git`, `msi-ec-dkms-git`).

## Ce qui démarre tout seul

| Composant | Quand | Mécanisme |
|---|---|---|
| module `msi_ec` (DKMS) | à chaque démarrage | `/usr/lib/modules-load.d/msi-ec.conf` ; le hook pacman `dkms` le recompile à chaque mise à jour du noyau |
| `msi-daemon` | à chaque démarrage | unité système activée par le preset `/usr/lib/systemd/system-preset/80-msi-center-linux.preset` (appliqué à l'installation : `systemctl preset`) |
| `clipd` | à chaque session, pour tous les utilisateurs | unité utilisateur activée par le preset `/usr/lib/systemd/user-preset/80-msi-center-linux.preset` (`systemctl --global preset`) |
| app `msi-center` | à chaque ouverture de session, en arrière-plan | `/etc/xdg/autostart/org.blazorde.MsiCenter.desktop` (désactivable dans **Réglages** : l'app écrit alors `~/.config/autostart/org.blazorde.MsiCenter.desktop` avec `Hidden=true`) |
| extension GNOME | à chaque connexion, **une fois activée** | `gnome-extensions enable msi-center@blazorde` (une fois par utilisateur) |

L'extension est installée pour tout le système (`/usr/share/gnome-shell/extensions/msi-center@blazorde`) mais
GNOME ne l'active pas sans l'utilisateur : la liste `org.gnome.shell enabled-extensions` est un réglage
personnel. Un fichier de valeurs par défaut dconf (`/etc/dconf/db/local.d/`) ne servirait à rien ici : il faut
un profil dconf `system-db:local` (absent par défaut sur Arch) et la valeur par défaut est ignorée dès que
l'utilisateur a déjà activé une autre extension. Le paquet ne touche donc pas à dconf ; le message
d'installation donne la commande.

Le paquet **ne démarre rien pendant l'installation** (le daemon réécrit l'EC, et le module chargé doit d'abord
être celui de DKMS) : on redémarre, ou on lance les commandes affichées par pacman.

L'activation D-Bus système de `msi-daemon` n'est pas installée : le daemon tourne dès le démarrage, et un
daemon arrêté à la main reste arrêté au lieu d'être relancé par le premier appel D-Bus.

## Installation en une fois

Prérequis : les en-têtes du noyau (`linux-cachyos-headers`, ou celles de chaque noyau installé), `base-devel`,
`rustup` (ou `rust`), et aucun paquet DKMS upstream `msi-ec` (`dkms status | grep -i msi`).

```bash
sudo pacman -S --needed base-devel git rustup dkms linux-cachyos-headers gtk4 libadwaita sqlite
rustup default stable                       # si rustup vient d'être installé

git clone git@github.com:Blazorde/msi-center-linux.git
cd msi-center-linux
scripts/package.sh                          # sans sudo : construit dist/*.pkg.tar.zst (quelques minutes)
sudo pacman -U dist/msi-center-linux-*.pkg.tar.zst dist/msi-ec-blazorde-dkms-*.pkg.tar.zst
gnome-extensions enable msi-center@blazorde # sans sudo, en session (une fois)
systemctl reboot
```

Ou depuis une release : télécharger les deux `.pkg.tar.zst` de la
[release GitHub](https://github.com/Blazorde/msi-center-linux/releases) puis `sudo pacman -U` dessus.

Après le redémarrage, vérifier :

```bash
modinfo -n msi_ec                    # …/updates/dkms/msi-ec.ko… (le module du projet)
cat /sys/module/msi_ec/version       # 0.13-blazorde.1
systemctl is-enabled msi-daemon      # enabled
systemctl status msi-daemon
systemctl --user is-enabled clipd    # enabled
msictl status
gnome-extensions info msi-center@blazorde   # State: ACTIVE
```

### Construire une version précise

```bash
scripts/package.sh                      # le commit courant (HEAD) ; les modifications non commitées sont ignorées
scripts/package.sh --ref tag=v0.1.0     # un tag (ou commit=<sha>, branch=<nom>)
scripts/package.sh -- --nocheck         # options supplémentaires passées à makepkg
```

`makepkg` tourne dans un dossier temporaire (`cargo fetch` dans `prepare()`, puis
`cargo build --release --locked --frozen`) ; seuls les paquets sont copiés dans `dist/` (ignoré par git).
Le script ne fait jamais `makepkg -i` : l'installation reste un `sudo pacman -U` explicite.

## Mise à jour

```bash
git pull && scripts/package.sh
sudo pacman -U dist/msi-center-linux-<ver>-*.pkg.tar.zst dist/msi-ec-blazorde-dkms-<ver>-*.pkg.tar.zst
systemctl reboot        # ou les commandes de redémarrage des services affichées par pacman
```

## Passer de l'installation de développement au paquet

`scripts/dev-install.sh --enable`, `scripts/app-install.sh` et `scripts/ext-install.sh` installent les mêmes
fichiers ailleurs (`/etc/modules-load.d`, `~/.local`) : les retirer **avant** d'installer le paquet, sinon
pacman refuse (`/usr/bin/msi-daemon existe déjà`) ou deux copies coexistent.

```bash
# 1. Système : arrête le daemon (tables d'usine puis mode auto), retire binaires, unités, polkit, D-Bus,
#    /etc/modules-load.d/msi-ec.conf et le paquet DKMS msi-ec-blazorde ajouté à la main.
sudo scripts/dev-install.sh --uninstall
dkms status | grep -i msi            # doit être vide (sinon : sudo dkms remove msi-ec-blazorde/<ver> --all)

# 2. Utilisateur (sans sudo) : l'app et son autostart dans ~/.local et ~/.config.
scripts/app-install.sh --uninstall

# 3. Extension installée dans ~/.local (elle masquerait celle du paquet).
gnome-extensions disable msi-center@blazorde
rm -rf ~/.local/share/gnome-shell/extensions/msi-center@blazorde

# 4. Vérifier qu'il ne reste rien.
ls /usr/bin/msi-daemon /usr/bin/msictl /usr/bin/clipd ~/.local/bin/msi-center 2>/dev/null   # rien
systemctl --global is-enabled clipd                                                          # disabled / not-found

# 5. Installer le paquet (section précédente), puis :
gnome-extensions enable msi-center@blazorde
systemctl reboot
```

Les réglages sont conservés : `/var/lib/msi-center/state.toml` (daemon), `~/.local/share/clipd/` (clips
épinglés), `~/.config/msi-center/` (app).

## Désinstallation

```bash
sudo pacman -Rns msi-center-linux msi-ec-blazorde-dkms
```

Avant la suppression, le paquet arrête `msi-daemon` : à l'arrêt, il réécrit les tables d'usine puis remet l'EC
en mode `auto` (S5, `ExecStopPost`). Le hook DKMS retire le module ; au prochain démarrage, le pilote `msi_ec`
du noyau (s'il existe) reprend la main. Les données listées ci-dessus restent à supprimer à la main.

## Installation de développement (sans paquet)

Pour itérer sans reconstruire de paquet, [`scripts/dev-install.sh`](../../scripts/dev-install.sh) installe le
daemon, `msictl` et `clipd` depuis `target/release/` (manifeste dans `/var/lib/msi-center/dev-install.manifest`) :

```bash
cargo build --release -p msi-daemon -p clipd     # sans sudo
sudo scripts/dev-install.sh --enable              # installe + démarre à chaque démarrage / session
sudo scripts/dev-install.sh --uninstall           # défait tout ce que le manifeste liste
```

`--enable` (idempotent, à relancer après chaque reconstruction) :

1. installe `kernel/msi-ec` en DKMS (paquet `msi-ec-blazorde`, recompilé à chaque nouveau noyau) et vérifie que
   `modinfo -n msi_ec` pointe vers `updates/dkms/` (le module du noyau porte le même nom : il n'est **pas** mis en
   liste noire, `depmod` donne la priorité à `updates/`) ;
2. écrit `/etc/modules-load.d/msi-ec.conf` (chargement au démarrage) ;
3. active `msi-daemon.service` et installe son activation D-Bus système (sans `--enable`, aucun appel D-Bus ne
   peut démarrer le daemon) ;
4. si `clipd` est construit : binaire, unité utilisateur, activation D-Bus de session et
   `systemctl --global enable clipd` (chaque session).

L'app s'installe dans `~/.local` avec [`scripts/app-install.sh`](app.md), l'extension avec
[`scripts/ext-install.sh --enable`](extension.md). Ne pas mélanger avec le paquet (voir la migration ci-dessus).
