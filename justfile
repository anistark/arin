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
# The title and the notes are both asked for rather than taken as arguments. `just publish`
# ends by cutting the release too, so an argument here is an argument there as well, threaded
# through a second recipe to reach one prompt. Asking puts the question in the one place that
# needs the answer, and both ways in get it.
#
# Answering `n` to the last prompt marks it a pre-release. What that buys is `/releases/latest`
# passing it over, which is the endpoint `arin --update-check` reads, so nobody on a stable
# build is told a pre-release is available. What it does not do is change the tag, hold back
# the dmg, or stop the tap job: `release.yml` fires on `v*` and opens the Homebrew pull request
# either way, so a pre-release still reaches `brew install` unless that job learns the
# difference.

# Tag the current version and cut a GitHub release. Asks for the title, notes, and stability.
gh-release:
    #!/usr/bin/env sh
    set -eu

    version=$(awk '/^\[workspace.package\]/{f=1} f&&/^version = /{gsub(/[",]/,"",$3); print $3; exit}' Cargo.toml)
    if [ -z "$version" ]; then
        echo "no version under [workspace.package]" >&2
        exit 1
    fi
    tag="v$version"

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

    # A default in the prompt rather than a numbered menu like the notes below, because there
    # are only two answers and return is one of them. A menu would ask twice to reach the
    # same place: pick "write your own", then write it.
    printf 'title [%s]: ' "$tag"
    read -r title
    [ -n "$title" ] || title="$tag"

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

    # Both a question and the last chance to call the run off, because the next line pushes
    # a tag. `y` and `n` each cut something, so anything else has to mean neither: return
    # answered no here before it meant a choice, and it must not now be the way somebody
    # publishes a release they were only reading the summary of.
    printf 'stable? y releases, n pre-releases, anything else stops: '
    read -r reply
    case "$reply" in
    y | Y) prerelease="" ;;
    n | N) prerelease="--prerelease" ;;
    *) exit 1 ;;
    esac

    git tag -a "$tag" -m "$title"
    git push origin "$tag"

    # Unquoted, like `$generate`, so an empty one disappears rather than arriving as an
    # empty argument.
    if [ -n "$generate" ]; then
        gh release create "$tag" --title "$title" $generate $prerelease
    elif [ -s "$notes_file" ]; then
        gh release create "$tag" --title "$title" --notes-file "$notes_file" $prerelease
    else
        gh release create "$tag" --title "$title" --notes "" $prerelease
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
    # Raising an application needs no permission. Everything else about a window needs
    # Accessibility, which Arin promises never to hold.
    hits=$(grep -rnE 'AXUIElement|AXIsProcessTrusted|kAXPosition|kAXSize|CGSMoveWindow|SLSMoveWindow' \
      --include='*.rs' crates/ | grep -vE ':[0-9]+:[[:space:]]*//' || true)
    if [ -n "$hits" ]; then
        echo "Accessibility API referenced. Arin activates and never arranges:" >&2
        echo "$hits" >&2
        exit 1
    fi
    echo "clean: no Accessibility APIs referenced"

# Green here means green there, which is the whole point of the recipe existing.
#
# The commands are the ones in `.github/workflows/ci.yml`, job for job, and the banners are
# that file's job names, so the two can be read side by side. The environment is the part
# that is easy to leave out and the part that decides the answer: the workflow sets
# `RUSTFLAGS: -D warnings` at the top level, so it reaches every job, and a warning out of
# rustc fails the build there in a test target as readily as in library code. `just test` on
# its own does not set it, deliberately, because an unused import should not stop you running
# the test you are halfway through writing. It should stop a push, and that is this recipe.
#
# Order is cheapest first, so a run that is going to fail fails in seconds rather than after
# a cold workspace build. CI has no order to match: these are four jobs and they run at once.
#
# What this cannot reach on its own is Linux, and Linux is half of what CI builds. The gap is
# not theoretical and it is not obscure: every `#[cfg(target_os = "macos")]` block in the tree
# has an other side that only Linux compiles, and a binding that block is the sole reader of is
# dead code over there. `-D warnings` turns dead code into a failed build, so the first machine
# to see it is a runner, in a pull request, after `just ci` said green.
#
# `just ci-linux` is that half, in a container, and the last step below runs it rather than
# leaving a note at the end for somebody to read. When Docker is down this says which jobs went
# unchecked instead of printing green. `just nix-check` closes none of it: another macOS build.

# Everything CI runs, under the environment CI runs it in.
ci:
    #!/usr/bin/env sh
    set -eu

    export RUSTFLAGS="-D warnings"
    export CARGO_TERM_COLOR=always

    echo "==> $(rustc --version)"
    echo "    just toolchain compares this against the stable CI would resolve"

    # A cargo outside the rustup shim reads no `rust-toolchain.toml`, so the pin this repo
    # carries binds nothing and the compiler below is whatever that install happens to be.
    # `just toolchain` explains it at length; this is one line so the run that is about to
    # answer "would CI be green" says up front which compiler it asked.
    if command -v rustup >/dev/null 2>&1; then
        cargo_path=$(command -v cargo)
        shim_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
        rustup_home=$(rustup show home 2>/dev/null || echo "${RUSTUP_HOME:-$HOME/.rustup}")
        case "$cargo_path" in
        "$shim_dir"/* | "$rustup_home"/*) ;;
        *) echo "    note: that cargo is $cargo_path, not the rustup shim, so rust-toolchain.toml binds nothing here. just toolchain" ;;
        esac
    fi

    echo "==> no input synthesis"
    just draw-only

    echo "==> fmt and clippy"
    cargo fmt --all --check
    cargo clippy --workspace --all-targets

    echo "==> core and protocol"
    cargo test -p arin-protocol -p arin-core --all-targets

    echo "==> workspace"
    cargo build --workspace --all-targets
    cargo test --workspace

    echo
    if [ "$(uname -s)" = Linux ]; then
        echo "green, and this machine is the platform the other half of CI runs on, so that"
        echo "was all of it."
    elif docker info >/dev/null 2>&1; then
        echo "green on $(uname -s). The Linux half is next."
        echo
        just ci-linux
    else
        echo "green on $(uname -s), which is half of what CI builds. The Linux half did not"
        echo "run: the workspace there, core with no platform crate in the tree, and the lint"
        echo "job. Start Docker and run just ci-linux, or push and find out in the pull request."
    fi

# The Linux half of CI, in a container, because the other side of a `#[cfg(target_os = "macos")]`
# is the one thing a Mac cannot compile and CI compiles it on every push. The three commands are
# `ci.yml`'s three Linux jobs, in the order `just ci` uses.
#
# Native architecture rather than CI's x86_64. `--platform linux/amd64` on an Apple Silicon
# machine is qemu, and minutes become tens of them. What that gives up is architecture specific,
# which this repo has none of. What it keeps is the target_os cfg, which is the entire reason to
# run this at all.
#
# The build goes to a named volume rather than ./target, and this is not tidiness. The container
# is root and its host build lands under the same `target/debug` name this machine uses, so a
# shared directory would leave root owned artifacts in the repo and make every native build
# afterwards a cold one. The registry gets a volume for the reverse reason: so the second run
# does not download the index again. The source is mounted writable because cargo expects to be
# able to touch Cargo.lock, and it is the one thing here that is not disposable.

# The Linux jobs from CI, in a container. Needs Docker running.
ci-linux:
    #!/usr/bin/env sh
    set -eu

    if [ "$(uname -s)" = Linux ]; then
        echo "this machine is already Linux, so just ci is the whole of it here." >&2
        exit 1
    fi
    if ! docker info >/dev/null 2>&1; then
        echo "docker is not running, and this recipe is a container." >&2
        exit 1
    fi

    echo "==> linux, in docker"
    docker run --rm -i \
        -v "$PWD:/src" \
        -w /src \
        -v arin-ci-linux-target:/ci/target \
        -v arin-ci-linux-registry:/usr/local/cargo/registry \
        -e CARGO_TARGET_DIR=/ci/target \
        -e RUSTFLAGS="-D warnings" \
        -e CARGO_TERM_COLOR=always \
        rust:latest sh -eus <<'CONTAINER'
    # rust-toolchain.toml asks for these and rustup would fetch them on first use anyway, but
    # only partway into a command that had already started printing. Asking first puts the
    # reason for the wait on screen before the wait.
    rustup component add clippy rustfmt
    echo "==> $(rustc --version), $(uname -m) linux"

    echo "==> fmt and clippy"
    cargo fmt --all --check
    cargo clippy --workspace --all-targets

    echo "==> core and protocol, with no platform crate in the tree"
    cargo test -p arin-protocol -p arin-core --all-targets

    echo "==> workspace"
    cargo build --workspace --all-targets
    cargo test --workspace
    CONTAINER

    echo
    echo "green on Linux too. Both halves of CI have now run."

# Neither side names a version. `rust-toolchain.toml` pins a channel, and CI's
# `dtolnay/rust-toolchain@stable` resolves the same channel, so both are whatever stable was
# on the day they asked. They drift anyway, because CI asks on every run and a laptop asks
# when somebody remembers to `rustup update`. A clippy lint that is six weeks old is the
# usual way that drift is discovered, and it is discovered in a pull request.
#
# The pin also only binds through the rustup shim. A cargo from Homebrew reads no
# `rust-toolchain.toml` at all and can sit a release either side of stable with nothing
# saying so, which is what the check below is looking for.

# Report the compiler this machine has, against the one CI would resolve.
toolchain:
    #!/usr/bin/env sh
    set -eu

    local_version=$(rustc --version | cut -d' ' -f2)

    echo "local"
    printf '  %-8s %s\n' \
        rustc "$(rustc --version)" \
        cargo "$(cargo --version)" \
        clippy "$(cargo clippy --version)" \
        rustfmt "$(cargo fmt --version)" \
        from "$(command -v cargo)"

    # A cargo outside the shim directory and outside the rustup home was found on PATH ahead
    # of rustup, by Homebrew or by a nix shell. Worth saying whether or not it agrees with the
    # pin today, because agreeing today is not the same as being held there.
    if command -v rustup >/dev/null 2>&1; then
        cargo_path=$(command -v cargo)
        shim_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
        rustup_home=$(rustup show home 2>/dev/null || echo "${RUSTUP_HOME:-$HOME/.rustup}")
        case "$cargo_path" in
        "$shim_dir"/* | "$rustup_home"/*) ;;
        *)
            pinned=$(rustup run stable rustc --version 2>/dev/null | cut -d' ' -f2 || true)
            echo
            echo "  That cargo is not the rustup shim, so rust-toolchain.toml binds nothing here."
            if [ -z "$pinned" ]; then
                echo "  rustup has no stable toolchain to compare it against."
            elif [ "$pinned" = "$local_version" ]; then
                echo "  It agrees with the pin today, at $pinned. Nothing holds it there."
            else
                echo "  The pin resolves to $pinned and this is $local_version, so it has drifted already."
            fi
            ;;
        esac
    fi

    echo
    echo "CI, dtolnay/rust-toolchain@stable, resolved fresh on every run"

    manifest=$(mktemp)
    trap 'rm -f "$manifest"' EXIT
    if ! curl -sSf --max-time 20 -o "$manifest" https://static.rust-lang.org/dist/channel-rust-stable.toml; then
        echo "  could not reach static.rust-lang.org, so there is nothing to compare against."
        exit 0
    fi

    ci_full=$(awk -F'"' '/^\[pkg\.rust\]/{f=1} f&&/^version = /{print $2; exit}' "$manifest")
    ci_version=${ci_full%% *}
    printf '  %-8s %s\n' rustc "rustc $ci_full"

    echo
    if [ "$ci_version" = "$local_version" ]; then
        echo "Same stable. just ci here runs the compiler CI runs."
    else
        echo "Different stable: $local_version here, $ci_version there. Close it with rustup update,"
        echo "or expect a lint CI has and this machine does not."
    fi
