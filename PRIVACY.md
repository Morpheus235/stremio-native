# Privacy

StremioNative collects nothing. It has no analytics, no crash reporting and no usage telemetry,
and it sends no data to its authors.

What leaves the TV, and where it goes:

| What | Sent to | When |
| --- | --- | --- |
| Your Stremio email and password | `api.strem.io` | only when you sign in |
| Your Stremio session key | `api.strem.io` | to read your installed addons and library |
| Catalog, search, meta, stream and subtitle requests, including your search text and the item you open (for subtitles also the playing file's name, size and hash, when its addon gave them) | the hosts of the addons you installed | while you browse, search or play |
| Subtitle file downloads | the hosts in a subtitles addon's links | when you pick an addon's subtitle, or your subtitle languages pick one |
| Media requests, with any headers an addon asked for | the hosts that serve the stream | while you play |
| Artwork requests | the hosts in the addon's image URLs | while you browse |
| A request for `/settings` (no data of yours beyond the request itself) | every host of your home network (the /24 of each of the TV's private IPv4 addresses) | only when you press Find on my network in Settings > Streams > Streaming server |
| A request for `/settings`, and a torrent stream's info hash and trackers | the streaming server you set in Settings > Streams | a check at app start, when you set the server and when you open its page; the stream while you play a torrent; only after you set a server |
| A trailer's YouTube ID (StremioNative itself sends nothing to YouTube; the helper that finishes the launch on webOS 3 keeps a short `trailer-helper.log` on the TV, like the other logs) | the TV's own YouTube app, so YouTube and Google learn which trailer you opened | only when you press Trailer on a title's page |
| A request for your region's programme guide (no data of yours beyond the request itself) | `epgshare01.online` | at most once a day, only while Settings > TV guide names a region (Off by default) |
| The ID of the episode or film you play (an IMDb, MyAnimeList or Kitsu ID) | `api.introdb.app`; for anime `api.aniskip.com`, `kitsu.io` to turn a Kitsu ID into a MyAnimeList one, and `arm.haglund.dev` to turn an IMDb ID and season into one when introdb.app has nothing | once per playback, to find its intro, recap and credits — never while Settings > Skip intros is Off |

Each addon is run by its own operator under its own terms; an addon's URL can carry your
configuration for it (for example a debrid key). StremioNative stores your session key and
settings on the TV only, protected by the TV's own key store, and removes them when you sign out.
Logs stay on the TV: the app has no code that uploads them, and nothing leaves the device unless
you copy a log off yourself (for example to attach it to a GitHub issue). Every line is scrubbed
before it is written: your session key, passwords, tokens, addon configuration in addon URLs,
keys inside stream links, server names and addresses, and search text are removed.
