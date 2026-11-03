# Releasing

## Homebrew Channel repository

`ci/hbrepo.py` turns a built ipk into a Homebrew Channel repository: `repo.json`, the package
manifest with sha256 and size, and the ipk URL. `.github/workflows/hbrepo.yml` publishes it on
GitHub Pages on every change to it on `main`; a release no longer refreshes it, so it is run by
hand after one. A TV adds `https://morpheus235.github.io/stremio-native/repo.json`
under Settings → Add repository and gets updates as releases appear.

The package itself is built by `make ipk` (`ci/mkipk.py`). It is reproducible and read back with
`--verify`. The launcher art (`pkg/`, plus the badged `pkg/beta/` and `pkg/debug/` sets) is drawn
by `ci/make-icons.py`.

## Cutting a release

Releases are tags on `main`, with no release branch. Bump `pkg/appinfo.json`'s version, then run
`git tag -a vX.Y.Z -m "StremioNative X.Y.Z" && git push origin vX.Y.Z`.

`release.yml` then:
1. checks that the tag matches the packaged version and is on `main`;
2. runs the tests;
3. cross-builds the ipk on an arm64 runner with the pinned webOS NDK;
4. publishes the GitHub release (ipk + sha256);
5. (the Homebrew Channel repository is refreshed by hand, `hbrepo.yml`, while it is on hold).

`ci.yml` runs on every push and pull request. It runs the host tests, the shipping feature set,
the packaging selftests and an ARM compile check, and does a full package build that is read back
but not published.

### Beta

A suffixed tag (`v0.2.0-beta.1`) builds the separate **StremioNative Beta** app instead:
- `FLAVOR=beta`, app id `com.morpheus235.stremionative.beta`;
- version `X.Y.YYMMDDHHm` (the UTC cut time to ten minutes), so at most one beta per ten minutes;
- published as a GitHub prerelease.

The one `repo.json` lists both apps, each pointing at its newest release, so either or both can
be installed.
