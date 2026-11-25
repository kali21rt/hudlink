#!/usr/bin/env bash
# Publish this repository to your GitHub account. Run once, from the hudlink/ folder.
set -euo pipefail
cd "$(dirname "$0")/.."

command -v gh >/dev/null || { echo "Install the GitHub CLI first:  sudo dnf install gh"; exit 1; }
gh auth status >/dev/null 2>&1 || gh auth login

NAME=$(git config --global user.name || true)
EMAIL=$(git config --global user.email || true)
if [[ -z "$NAME" || -z "$EMAIL" ]]; then
  echo "Set your git identity first:"
  echo "  git config --global user.name  \"Your Name\""
  echo "  git config --global user.email \"you@example.com\"   (use the email of your GitHub account)"
  exit 1
fi

echo "==> 1/5 Put your name on the commits (their dates stay as they are)"
if git log --format='%ae' | grep -q 'hudlink@localhost'; then
  export HN="$NAME" HE="$EMAIL"
  FILTER_BRANCH_SQUELCH_WARNING=1 git filter-branch -f --env-filter '
    export GIT_AUTHOR_NAME="$HN" GIT_AUTHOR_EMAIL="$HE" GIT_COMMITTER_NAME="$HN" GIT_COMMITTER_EMAIL="$HE"' -- --all
  git for-each-ref --format='%(refname)' refs/original/ | xargs -r -n1 git update-ref -d
fi
git log --format='  %h %ad %an  %s' --date=short

# fill in your GitHub username in the README links
GH_USER=$(gh api user -q .login)
if grep -q '<your-username>\|\.\./\.\./actions' README.md; then
  sed -i "s|<your-username>|$GH_USER|g; s|\.\./\.\./actions|https://github.com/$GH_USER/hudlink/actions|g" README.md
  git commit -qam "README: point links to $GH_USER/hudlink"
fi

echo "==> 2/5 License"
if [[ ! -f LICENSE ]]; then
  curl -fsSL https://www.gnu.org/licenses/gpl-3.0.txt -o LICENSE
  git add LICENSE && git commit -q -m "Add GPL-3.0 license text"
fi

echo "==> 3/5 Screenshots"
if ! ls docs/images/*.png >/dev/null 2>&1; then
  echo "   docs/images has no screenshots yet (run scripts/screenshots.sh, add a photo as docs/images/hero.jpg)."
  read -rp "   Publish anyway? [y/N] " a; [[ "$a" == [yY]* ]] || exit 1
elif [[ -n "$(git status --porcelain docs/images)" ]]; then
  git add docs/images && git commit -q -m "Add screenshots and photos"
fi

echo "==> 4/5 Create the GitHub repository and push"
gh repo create hudlink --public --source . --remote origin --push \
  --description "A tiny ESP32 status display for Linux: system monitor, cava, synced lyrics, matugen colors. Rust daemon + ESP32 firmware."
gh repo edit --add-topic esp32,rust,linux,cava,matugen,ili9341,system-monitor,lyrics,tft-display

echo "==> 5/5 Tag the first release (GitHub Actions builds the binary and firmware)"
git tag -a v2.1.0 -m "hudlink 2.1.0"
git push origin v2.1.0
echo
echo "Done: $(gh repo view --json url -q .url)"
