# Arin task runner. `just` with no arguments lists everything.

# Show available recipes.
default:
    @just --list

# development
# Run the daemon with no renderer. The protocol works, but nothing is drawn.
dev:
    cargo run --bin arin -- daemon --headless

# Drive a running daemon. `just run point 412 88 --display 1 --label Save`
run *ARGS:
    @cargo run --quiet --bin arin -- {{ ARGS }}

# Fastest feedback loop: typecheck without codegen.
check:
    cargo check --workspace --all-targets

build:
    cargo build --workspace --all-targets

release:
    cargo build --workspace --release

# packaging
# Universal when both darwin targets are installed, native when they are not. Pass an
# identity to sign it: `just bundle --sign "Developer ID Application: ..."`.

# Build Arin.app into target/bundle.
bundle *ARGS:
    packaging/macos/bundle.sh {{ ARGS }}

# Start Arin at login, from the bundle so the Screen Recording grant sticks.
startup-enable app="/Applications/Arin.app":
    packaging/macos/launch-agent.sh enable {{ app }}

# Stop starting Arin at login. Leaves the app alone.
startup-disable:
    packaging/macos/launch-agent.sh disable

# The flake builds the same bundle without a Rust toolchain on the machine. macOS only,
# and it needs Nix, which is why neither recipe is part of `just ci`.

# Build Arin.app through the flake, into ./result.
nix-build:
    nix build --print-build-logs

# Build it both ways, with the workspace tests run inside the build and without.
nix-check:
    nix flake check --print-build-logs

test:
    cargo test --workspace

# Open the API docs for every crate.
doc:
    cargo doc --workspace --no-deps --open

# docs site
# The Eleventy site in docs/ that GitHub Pages publishes. Both recipes install on first
# run and rebuild on change.

# Serve the docs site at http://localhost:8080.
docs:
    pnpm --dir docs install
    pnpm --dir docs dev

# Serve the docs site on every interface, and print the LAN and tailscale URLs.
docs-host:
    pnpm --dir docs install
    DOCS_SHOW_HOSTS=1 pnpm --dir docs dev

clean:
    cargo clean

# quality
alias format := fmt

fmt:
    cargo fmt --all

# Formatting and clippy, at the same strictness CI uses.
lint:
    cargo fmt --all --check
    RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets

# versioning
# Every crate already inherits `version.workspace`, so the only literals that can drift
# are the path dependencies in `[workspace.dependencies]`, which need a version alongside
# the path to be publishable. Bump `[workspace.package]` and run this.

# Point every local dependency at the workspace version.
sync-version:
    #!/usr/bin/env sh
    set -eu
    version=$(awk '/^\[workspace.package\]/{f=1} f&&/^version = /{gsub(/[",]/,"",$3); print $3; exit}' Cargo.toml)
    if [ -z "$version" ]; then
        echo "no version under [workspace.package]" >&2
        exit 1
    fi

    # A crate that names its own version instead of inheriting is a drift this cannot fix.
    stray=$(grep -L '^version.workspace = true' crates/*/Cargo.toml || true)
    if [ -n "$stray" ]; then
        echo "these do not inherit the workspace version, so syncing would not reach them:" >&2
        echo "$stray" >&2
        exit 1
    fi

    before=$(grep -cE '^arin[a-z-]* = \{ path = "crates/[a-z-]+", version = "'"$version"'" \}' Cargo.toml || true)
    sed -E 's|^(arin[a-z-]* = \{ path = "crates/[a-z-]+", version = )"[^"]*"|\1"'"$version"'"|' \
        Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml
    total=$(grep -cE '^arin[a-z-]* = \{ path = "crates/' Cargo.toml)

    # Proves the manifest still parses and that every crate now reports the same version.
    cargo metadata --no-deps --format-version 1 >/dev/null
    echo "workspace version $version, $total local dependencies in sync ($((total - before)) rewritten)"

# release
# Cut a GitHub release for the version in Cargo.toml, and let CI attach the built app.
#
# Order matters and is not arbitrary. The tag is pushed first, because that is what fires
# `.github/workflows/release.yml`, and the release is created immediately after so the
# notes written here are the ones that survive: that workflow attaches to an existing
# release rather than creating a second one, but it will write its own notes if it gets
# there first. It spends minutes building before it looks, so it never does.
#
# `just gh-release` titles the release `v{version}`. Pass a title to override:
# `just gh-release "Arin, now installable"`.

# Tag the current version and cut a GitHub release. Asks where the notes come from.
gh-release title="":
    #!/usr/bin/env sh
    set -eu

    version=$(awk '/^\[workspace.package\]/{f=1} f&&/^version = /{gsub(/[",]/,"",$3); print $3; exit}' Cargo.toml)
    if [ -z "$version" ]; then
        echo "no version under [workspace.package]" >&2
        exit 1
    fi
    tag="v$version"
    title="{{ title }}"
    [ -n "$title" ] || title="$tag"

    # Each guard below is something that cannot be undone from here. A tag points at a
    # commit forever, and a release cut from a tree that does not match what was tested is
    # a release nobody can reproduce.
    if [ -n "$(git status --porcelain)" ]; then
        echo "working tree is dirty. Commit or stash before tagging." >&2
        exit 1
    fi
    if gh release view "$tag" >/dev/null 2>&1; then
        echo "release $tag already exists. Bump [workspace.package].version first." >&2
        exit 1
    fi
    if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
        echo "tag $tag already exists locally. Delete it or bump the version." >&2
        exit 1
    fi
    branch=$(git rev-parse --abbrev-ref HEAD)
    if [ "$branch" != "main" ]; then
        printf 'on branch %s, not main. Continue? [y/N] ' "$branch"
        read -r reply
        case "$reply" in y | Y) ;; *) exit 1 ;; esac
    fi

    notes_file=$(mktemp)
    trap 'rm -f "$notes_file"' EXIT

    echo "Release notes for $tag:"
    echo "  1) generated from commits since the last release"
    echo "  2) from CHANGELOG.md"
    echo "  3) write your own"
    echo "  4) none"
    printf 'choose [1]: '
    read -r choice
    [ -n "$choice" ] || choice=1

    generate=""
    case "$choice" in
    1)
        generate="--generate-notes"
        ;;
    2)
        # The version's own section if there is one, and [Unreleased] otherwise, since
        # nothing is tagged before 1.0 and everything accumulates there until then.
        awk -v v="$version" '
            $0 ~ "^## \\[" v "\\]" { f = 1; next }
            f && /^## / { exit }
            f { print }
        ' CHANGELOG.md > "$notes_file"
        if [ ! -s "$notes_file" ]; then
            awk '
                /^## \[Unreleased\]/ { f = 1; next }
                f && /^## / { exit }
                f { print }
            ' CHANGELOG.md > "$notes_file"
            echo "no [$version] section, using [Unreleased]"
        fi
        if [ ! -s "$notes_file" ]; then
            echo "nothing to read out of CHANGELOG.md" >&2
            exit 1
        fi
        ;;
    3)
        printf '\n' > "$notes_file"
        "${EDITOR:-vi}" "$notes_file"
        if [ ! -s "$notes_file" ]; then
            echo "empty notes, nothing written" >&2
            exit 1
        fi
        ;;
    4) : ;;
    *)
        echo "not an option: $choice" >&2
        exit 1
        ;;
    esac

    echo
    echo "  tag    $tag at $(git rev-parse --short HEAD)"
    echo "  title  $title"
    printf 'cut it? [y/N] '
    read -r reply
    case "$reply" in y | Y) ;; *) exit 1 ;; esac

    git tag -a "$tag" -m "$title"
    git push origin "$tag"

    if [ -n "$generate" ]; then
        gh release create "$tag" --title "$title" $generate
    elif [ -s "$notes_file" ]; then
        gh release create "$tag" --title "$title" --notes-file "$notes_file"
    else
        gh release create "$tag" --title "$title" --notes ""
    fi

    echo
    echo "CI is building the app. The dmg and checksums attach themselves when it lands:"
    echo "  gh run watch"

# Publish the crates, then cut the release.
#
# Only `arin-protocol` and `arin` go to crates.io; every other crate is `publish = false`
# and `--workspace` skips them without being told.
#
# `--workspace` rather than one `cargo publish -p` per crate, and the difference is not
# stylistic. `arin` depends on `arin-protocol` at the same version, so dry running it on
# its own resolves that dependency against the real index and fails until the protocol is
# already up there:
#
#     failed to select a version for the requirement `arin-protocol = "^0.2.0"`
#     candidate versions found which didn't match: 0.1.0
#
# Publishing the protocol first to get past that means uploading it before the facade has
# been verified at all, and an upload cannot be undone: a version can be yanked but never
# reused. `--workspace` avoids the trade entirely by verifying the facade against a
# temporary registry holding the freshly packaged protocol, so both are checked before
# either is uploaded and cargo orders the uploads itself.

# Publish `arin-protocol` and `arin` to crates.io, then cut the release.
publish: test lint
    #!/usr/bin/env sh
    set -eu

    version=$(awk '/^\[workspace.package\]/{f=1} f&&/^version = /{gsub(/[",]/,"",$3); print $3; exit}' Cargo.toml)
    echo "==> publishing arin-protocol and arin at $version"

    # `gh-release` checks both of these too, and it runs at the end, by which point the
    # crates are on crates.io and cannot be taken back. A run that uploads and then refuses
    # to tag leaves the two halves of a release disagreeing, so check first.
    if [ -n "$(git status --porcelain)" ]; then
        echo "working tree is dirty. Commit or stash before publishing." >&2
        exit 1
    fi
    if gh release view "v$version" >/dev/null 2>&1; then
        echo "release v$version already exists. Bump the version first." >&2
        exit 1
    fi

    cargo publish --workspace --dry-run

    printf '\ndry run passed for both. Upload to crates.io? This cannot be undone. [y/N] '
    read -r reply
    case "$reply" in y | Y) ;; *) exit 1 ;; esac

    cargo publish --workspace

    just gh-release

# invariants
# CI runs this on Linux, where a platform crate in the tree would fail to build.
# Locally it still catches a platform dependency leaking into core.

# Check core and the protocol stand alone, with no platform crate.
core:
    cargo test -p arin-protocol -p arin-core --all-targets

# Comment lines are skipped, so the rule can be written down next to the code it
# governs without tripping the check on its own wording.

# Check the product boundary: Arin draws and never actuates.
draw-only:
    #!/usr/bin/env sh
    hits=$(grep -rnE 'CGEventPost|CGEventTap|CGEventCreateMouseEvent|CGEventCreateKeyboardEvent|SendInput|XTestFake|uinput' \
      --include='*.rs' crates/ | grep -vE ':[0-9]+:[[:space:]]*//' || true)
    if [ -n "$hits" ]; then
        echo "input synthesis API referenced. Arin draws and never actuates:" >&2
        echo "$hits" >&2
        exit 1
    fi
    echo "clean: no input synthesis APIs referenced"

# Everything CI runs, in the order it runs it. Green here means green there.
ci: lint core test draw-only
