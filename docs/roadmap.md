# Roadmap

## 1. Foundation (in progress)
Layer crates ported and renamed (`sn_*`): base, machine, net, platform, gfx, ui, storage.
Plex-specific transport behaviour removed from `net`; the bundled-roots TLS fallback applies to
any HTTPS host (old TVs lack the ISRG roots most addons chain to).

## 2. Media
Port the FFmpeg demuxer, curl pull source, HLS, libass subtitles and the Starfish player engine.
The Plex transcode decision (`route/`) and transcode ABR are not carried over.

- **DTS plays natively.** Feed the DTS core to the TV's decoder when its codec table lists DTS
  and the stream's channel count fits the measured ceiling (DTS-HD extensions are stripped to the
  core). Otherwise decode in software and feed PCM/AC3: there is no server to transcode it.
- **Never hang on audio the pipeline cannot render.** A stalled start falls back instead of
  spinning forever; a video-only stream plays silently.

## 3. Stremio addon layer
- Manifest, catalog, meta, stream and subtitle resources; Cinemeta first.
- **Complete catalogs.** Page with the `skip` extra until an addon returns a short page. Per-addon
  timeouts and failures are surfaced, never a silently short list.
- **Search every searchable catalog**, of any type (TV/channel catalogs such as TvVoo's, not only
  movie/series), per addon, with a filter to choose which addons are queried.
- **Per-addon results are signalled.** An addon with more hits than one page shows that more
  exist and loads them on demand.
- **TvVoo streams load.** Honour `behaviorHints` (`proxyHeaders` request headers, `notWebReady`,
  `bingeGroup`) and the stream URL forms live-TV addons return.
- **Viewport-first loading.** Catalog rows and artwork load for what is on screen first; requests
  for rows scrolled past are cancelled or deprioritised and rescheduled, never drained top-down.

## 4. App and screens
Boot, home rows, detail with streams, library, settings, player.

- **Search** with loading, empty and error states (a spinner while a query is in flight).
- **Hotkeys**: move left/right through the top bar, jump to Search and Settings from anywhere.

## Distribution: Homebrew Channel repository
`ci/hbrepo.py` turns a built .ipk into a Homebrew Channel repository (`repo.json`, the package
manifest with sha256 and size, and the ipk). `.github/workflows/hbrepo.yml` publishes it on
GitHub Pages for every release, so a TV adds
`https://morpheus235.github.io/stremio-native/repo.json` under Settings > Add repository and
receives updates as new releases appear. Still needed: the ipk packaging step (`mkipk`) and the
launcher icons (`pkg/icon160.png` and friends).
