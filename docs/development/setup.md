# Installer l'environnement de dev (CachyOS)

## Paquets

```bash
sudo pacman -S --needed base-devel rustup github-cli gtk4 libadwaita sqlite dkms linux-cachyos-headers wl-clipboard
rustup default stable
rustup component add rustfmt clippy
```

## GitHub

```bash
gh auth login        # choisir GitHub.com, HTTPS, navigateur
```

## Activer les hooks et vérifier

```bash
cargo install cargo-deny --locked     # requis par scripts/ci.sh
git config core.hooksPath .githooks
scripts/ci.sh
```

## Outils conseillés

- `cargo install cargo-nextest` — tests plus rapides
- `d-spy` (Flathub) — inspecter les bus D-Bus
- Extension GNOME : `journalctl -f -o cat /usr/bin/gnome-shell` pour les logs ;
  sous Wayland, tester dans une session imbriquée : `dbus-run-session gnome-shell --devkit`
