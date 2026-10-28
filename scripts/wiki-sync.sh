#!/usr/bin/env bash
# Publishes docs/ to the GitHub wiki (https://github.com/Blazorde/msi-center-linux/wiki):
#
#   scripts/wiki-sync.sh                     # committed HEAD of docs/ → wiki, commit + push
#   scripts/wiki-sync.sh --dry-run [DIR]     # generate the pages only (default: a temporary directory)
#
# docs/ stays the source of truth: the wiki is regenerated from scratch (flat page names, rewritten links,
# images under images/, _Sidebar.md, _Footer.md), never edited by hand. Uncommitted changes are NOT published
# (a warning says so). The wiki repository must exist (first page created once in the GitHub web UI).
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
wiki_url="https://github.com/Blazorde/msi-center-linux.wiki.git"
dry_run=0
out=""

while (($#)); do
    case $1 in
        --dry-run) dry_run=1; out=${2:-}; shift $(($# > 1 ? 2 : 1)) ;;
        -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
        *) echo "option inconnue : $1" >&2; exit 2 ;;
    esac
done

command -v python3 >/dev/null || { echo "python3 absent." >&2; exit 1; }
[[ -z $(git -C "$repo" status --porcelain -- docs) ]] ||
    echo "Attention : docs/ a des modifications non commitées, elles ne seront pas publiées." >&2

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
sha="$(git -C "$repo" rev-parse --short HEAD)"
mkdir "$tmp/src"
git -C "$repo" archive HEAD docs | tar -x -C "$tmp/src"

if ((dry_run)); then
    if [[ -z $out ]]; then
        out="$(mktemp -d)/wiki"
    fi
else
    git clone --quiet "$wiki_url" "$tmp/wiki" ||
        { echo "Wiki introuvable : créer la première page sur GitHub (onglet Wiki), puis relancer." >&2; exit 1; }
    out="$tmp/wiki"
fi

python3 -I - "$tmp/src/docs" "$repo" "$out" <<'PY'
import os
import re
import shutil
import sys
from pathlib import Path, PurePosixPath

DOCS, REPO, OUT = (Path(a) for a in sys.argv[1:4])
BLOB = "https://github.com/Blazorde/msi-center-linux/blob/main/"
TREE = "https://github.com/Blazorde/msi-center-linux/tree/main/"

# Wiki page names are flat and global: one explicit name per page, ADRs named after their number.
PAGES = {
    "README.md": "Home",
    "product/roadmap.md": "Roadmap",
    "product/release-checklist.md": "Checklist-de-release",
    "architecture/overview.md": "Architecture",
    "architecture/dbus-api.md": "API-D-Bus",
    "adr/README.md": "ADR",
    "hardware/stealth-14-a1vgg.md": "Materiel-Stealth-14-A1VGG",
    "hardware/ec-map.md": "Carte-EC",
    "hardware/fan-safety.md": "Securite-ventilateurs",
    "hardware/fan-curve-sysfs.md": "Interface-sysfs-courbes",
    "hardware/gpu-power.md": "Alimentation-GPU-et-telemetrie",
    "development/setup.md": "Environnement-de-dev",
    "development/git-workflow.md": "Workflow-git",
    "user/installation.md": "Installation",
    "user/app.md": "Application",
    "user/extension.md": "Extension-GNOME",
    "design/app-liquid-glass.md": "Design-de-l-app",
}
for p in sorted((DOCS / "adr").glob("[0-9]*.md")):
    PAGES[f"adr/{p.name}"] = "ADR-template" if p.name.startswith("0000") else f"ADR-{p.name[:4]}"

missing = sorted(str(p.relative_to(DOCS)) for p in DOCS.rglob("*.md") if str(p.relative_to(DOCS)) not in PAGES)
if missing:
    sys.exit(f"pas de nom de page wiki pour {missing} : l'ajouter dans PAGES (scripts/wiki-sync.sh)")

LINK = re.compile(r"(!?)\[([^\]]*)\]\(([^)\s]+)\)")


def rewrite(src_rel, text):
    base = PurePosixPath(src_rel).parent

    def sub(m):
        bang, label, target = m.groups()
        if re.match(r"^[a-z]+:", target) or target.startswith("#"):
            return m.group(0)
        path, _, anchor = target.partition("#")
        norm = os.path.normpath(str(base / path)) if path else src_rel
        frag = f"#{anchor}" if anchor else ""
        if norm.startswith(".."):  # outside docs/: link to the file on GitHub
            repo_path = os.path.normpath("docs/" + norm)
            url = (TREE if (REPO / repo_path).is_dir() else BLOB) + repo_path
            return f"{bang}[{label}]({url}{frag})"
        if norm in PAGES:
            return f"{bang}[{label}]({PAGES[norm]}{frag})"
        if (DOCS / norm).is_file():  # image or other asset, copied under images/
            return f"{bang}[{label}](images/{norm}{frag})"
        if (DOCS / norm).is_dir():
            return f"{bang}[{label}]({TREE}docs/{norm})"
        sys.exit(f"docs/{src_rel} : lien introuvable {target}")

    return LINK.sub(sub, text)


OUT.mkdir(parents=True, exist_ok=True)
for child in OUT.iterdir():
    if child.name != ".git":
        shutil.rmtree(child) if child.is_dir() else child.unlink()

for rel, page in PAGES.items():
    (OUT / f"{page}.md").write_text(rewrite(rel, (DOCS / rel).read_text()))

for asset in DOCS.rglob("*"):
    if asset.is_file() and asset.suffix != ".md":
        dst = OUT / "images" / asset.relative_to(DOCS)
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(asset, dst)

adrs = "\n".join(
    f"  - [{(DOCS / rel).read_text().splitlines()[0].lstrip('# ')}]({page})"
    for rel, page in PAGES.items()
    if page.startswith("ADR-0")
)
(OUT / "_Sidebar.md").write_text(f"""**[🏠 Accueil](Home)**

**📖 Utilisateur**
- [Installation](Installation)
- [Application](Application)
- [Extension GNOME](Extension-GNOME)
- [Design de l'app](Design-de-l-app)

**🔧 Matériel**
- [Stealth 14 A1VGG](Materiel-Stealth-14-A1VGG)
- [Carte EC](Carte-EC)
- [Sécurité ventilateurs](Securite-ventilateurs)
- [Interface sysfs](Interface-sysfs-courbes)
- [GPU & télémétrie](Alimentation-GPU-et-telemetrie)

**🏗️ Architecture**
- [Vue d'ensemble](Architecture)
- [API D-Bus](API-D-Bus)
- [ADR](ADR)
{adrs}

**👩‍💻 Développement**
- [Environnement de dev](Environnement-de-dev)
- [Workflow git](Workflow-git)

**🎯 Produit**
- [Roadmap](Roadmap)
- [Checklist de release](Checklist-de-release)
""")
(OUT / "_Footer.md").write_text(
    f"Généré depuis [`docs/`]({TREE}docs) par `scripts/wiki-sync.sh` — modifier la doc dans le dépôt, pas ici.\n"
)
print(f"{len(PAGES)} pages → {OUT}")
PY

if ((dry_run)); then
    echo "Aperçu (rien publié) : $out"
    exit 0
fi

cd "$out"
git add -A
if git diff --cached --quiet; then
    echo "Wiki déjà à jour avec docs/ ($sha)."
    exit 0
fi
git commit --quiet -m "docs: sync wiki from docs/ ($sha)"
git push --quiet origin HEAD
echo "Wiki publié depuis $sha : ${wiki_url%.wiki.git}/wiki"
