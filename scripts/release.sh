#!/usr/bin/env bash
# Publish the current commit as the release, for the Mac and for Windows.
#
#   ./scripts/release.sh notes.md [Voi-<version>.<build>.exe]
#
# The source repository is private; the builds go to the public repository
# named in config/releases-repo, where anyone can download them and where the
# app itself looks for updates (crates/voi-core/src/update.rs). The files are
# named for what they are, Voi-0.1.0.65.dmg and Voi-0.1.0.65.exe, under the tag
# v0.1.0.65.
#
# The Windows setup is built on a Windows machine by
# scripts/package-windows.ps1 from this same commit (given VOI_COMMIT and
# VOI_BUILD when the tree there has no history). Its name says which build it
# is, and one from another build is refused.
#
# There is only ever one release there: this one. It is created first and
# marked Latest, and only once it exists is whatever was there before removed,
# tag and all. A failed upload therefore leaves the previous release standing
# rather than nothing. https://github.com/<releases>/releases/latest always
# shows it.
#
# A release is a commit, so the tree has to be clean and the commit pushed: a
# build made from uncommitted work would be published under a commit that does
# not contain it.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NOTES="${1:-}"
SETUP="${2:-}"
[ -n "$NOTES" ] && [ -f "$NOTES" ] || { echo "usage: $0 notes.md [Voi-<version>.<build>.exe]" >&2; exit 1; }
[ -z "$SETUP" ] || [ -f "$SETUP" ] || { echo "no such setup: $SETUP" >&2; exit 1; }

cd "$ROOT"
git diff --quiet HEAD || { echo "uncommitted changes — commit first; a release is a commit" >&2; exit 1; }

SOURCE=$(git remote get-url origin | sed -E 's#^(https://([^@]+@)?github.com/|git@github.com:)##; s#\.git$##')
RELEASES=$(tr -d '[:space:]' < "$ROOT/config/releases-repo")

# The same numbers bundle.sh writes into the app, so the release and the app it
# contains agree about what they are.
VERSION=$(awk -F'"' '/^version = /{print $2; exit}' "$ROOT/Cargo.toml")
BUILD=$(( $(git rev-list --count HEAD) + $(tr -d '[:space:]' < "$ROOT/config/build-base") ))
NAME="Voi-$VERSION.$BUILD"
TAG="v$VERSION.$BUILD"
TITLE="Voi $VERSION (build $BUILD)"

if [ -n "$SETUP" ] && [ "$(basename "$SETUP")" != "$NAME.exe" ]; then
    echo "the setup is $(basename "$SETUP"), not $NAME.exe: built from another commit?" >&2
    exit 1
fi

# Whichever logged-in gh account can write to both repositories. Asked rather
# than assumed: the account that is active in gh is not necessarily the one
# with access, and hard-coding a name breaks the day that changes. The token
# only ever lives in this variable; it is not printed.
can_push() { [ "$(GH_TOKEN="$1" gh api "repos/$2" --jq .permissions.push 2>/dev/null)" = "true" ]; }
GH_TOKEN=""
for user in $(gh auth status 2>&1 | sed -n 's/.*account \([^ ]*\).*/\1/p'); do
    token=$(gh auth token --user "$user" 2>/dev/null || true)
    [ -n "$token" ] || continue
    if can_push "$token" "$SOURCE" && can_push "$token" "$RELEASES"; then
        GH_TOKEN="$token"
        echo "▸ publishing to $RELEASES as $user"
        break
    fi
done
[ -n "$GH_TOKEN" ] || { echo "no logged-in gh account can write to $SOURCE and $RELEASES" >&2; exit 1; }
export GH_TOKEN

echo "▸ pushing"
git push -q origin HEAD

"$ROOT/scripts/dmg.sh" >/dev/null

# Under their release names, whatever the files are called here: an asset's
# name is its file's.
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp "$ROOT/target/Voi.dmg" "$STAGE/$NAME.dmg"
ASSETS=("$STAGE/$NAME.dmg")
if [ -n "$SETUP" ]; then
    cp "$SETUP" "$STAGE/$NAME.exe"
    ASSETS+=("$STAGE/$NAME.exe")
fi

echo "▸ creating $TAG"
gh release create "$TAG" "${ASSETS[@]}" \
    --repo "$RELEASES" \
    --title "$TITLE" \
    --notes-file "$NOTES" \
    --latest >/dev/null

# Only now, with the new one in place.
for old in $(gh release list --repo "$RELEASES" --json tagName --jq '.[].tagName'); do
    [ "$old" = "$TAG" ] && continue
    echo "▸ removing $old"
    gh release delete "$old" --repo "$RELEASES" --cleanup-tag --yes
done

echo
echo "Published $TITLE."
echo "  release:  https://github.com/$RELEASES/releases/tag/$TAG"
echo "  latest:   https://github.com/$RELEASES/releases/latest"
