# Roadmap

## Done

- **Foundation.** The PlxNative layers, ported as `sn_*` crates: base, machine, net, platform,
  gfx, ui, storage. Plex-specific transport is gone. The bundled-roots TLS fallback applies to any
  HTTPS host, because old TVs lack the roots most addons chain to.
- **Addons.** Manifest, catalog, meta, stream and subtitle resources, with Cinemeta first.
  Catalogs page with `skip` until a short page comes back. Each addon's timeouts and failures are
  shown, never a silently short list. `behaviorHints` are honoured (`proxyHeaders`,
  `notWebReady`, `bingeGroup`), so live-TV addons such as TvVoo play.
- **Viewport-first loading** (`crates/stremio/src/sched.rs`). The nearest request starts first,
  and work scrolled out of view is cancelled.
- **Account.** Sign-in, a kept session, the account's addons, Continue Watching, Library and
  watched state.
- **Screens.**
  - Home: one row per catalog.
  - Discover, Library, Continue Watching, Live TV, Settings.
  - Search: every catalog with the `search` extra, of any type, with an addon filter, a
    loading/empty/error state per row and recent searches.
  - Detail: episodes by season, and each addon's streams with its own loading state.
  - Top-bar and colour-key shortcuts.
  - Seven dark themes (Settings → Theme), OLED black among them, switched at run time.
  - Posters or a list on Home, Discover, Search, the Library and Continue Watching; a list row
    shows the year, genres, plot and IMDb rating.
  - Live TV's TV guide (Settings → TV guide): now / next on channel cards and in the player,
    from a region's public XMLTV guide matched by channel name (`crates/stremio/src/epg.rs`).
  - Live TV's Recently watched: the channels the account played, newest first, as the first
    choice of the Live TV catalog picker (a channel has no position, so Continue Watching never
    lists it).
- **Player.**
  - Probe before Load: the bundled demuxer reads the container first, so the Load names the
    real codecs.
  - Hardware playback over the Starfish buffer feed. HLS.
  - Dolby Vision Profile 7 is played as 8.1 (`crates/media/src/dovi.rs`).
  - libass text subtitles with embedded fonts, plus addon subtitles.
  - Track choice, Up Next, and progress saved to the account.
  - Skip intro/recap/credits, from chapter names or introdb (by IMDb id, also for `tmdb:` items).
  - CH +/- jumps between chapters.
- **Audio the TV can't decode.**
  - Software decode feeds S16LE 48 kHz PCM: TrueHD, DTS, FLAC, LPCM, Opus, Vorbis, MP2, WMA, ALAC.
  - Multichannel goes out as AC-3 640 kb/s 5.1 by default on every output, eARC included: on the
    C1 (webOS 6) app PCM is mixed down to stereo inside the TV even with eARC and Pass Through
    (issue #3), while a Dolby bitstream reaches the soundbar as 5.1.
  - Settings → Multichannel audio can choose Dolby Digital Plus (E-AC-3 768 kb/s 5.1, for HDMI
    ARC/eARC), multichannel PCM (not confirmed as 5.1 on any model yet), plain stereo, or Pro
    Logic II.
  - Verified on the C1 over optical with all 5.1 channels in place: AC-3, E-AC-3, AAC, TrueHD,
    DTS, FLAC, PCM.
  - webOS 5/6 refuse a DTS Load, so DTS always goes through software.

## Open

- Player freeze on exit at the end of an episode, or on resuming one. Under investigation: the
  previous session's log is kept as `stremionative-events.prev.log`.
- Stereo and Pro Logic II don't cover AC-3/E-AC-3 tracks yet. The bundled FFmpeg has no ac3/eac3
  decoder, so those stay native.
- Automatic could pick stereo for TV speakers, headphones and Bluetooth output.
- 7.1 PCM over eARC (5.1 is the ceiling today).
- A TvVoo channel that starts without sound.
- Contrast pass on the episode list, the selected episode and the stream list.

How releases are cut and published: [releasing.md](releasing.md).
