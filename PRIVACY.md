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
| The ID of the episode or film you play (an IMDb, MyAnimeList or Kitsu ID) | `api.introdb.app`; for anime `api.aniskip.com`, `kitsu.io` to turn a Kitsu ID into a MyAnimeList one, and `arm.haglund.dev` to turn an IMDb ID and season into one when introdb.app has nothing | once per playback, to find its intro, recap and credits — never while Settings > Skip intros is Off |

Each addon is run by its own operator under its own terms; an addon's URL can carry your
configuration for it (for example a debrid key). StremioNative stores your session key and
settings on the TV only, protected by the TV's own key store, and removes them when you sign out.
Logs stay on the TV, and every line is scrubbed before it is written: your session key,
passwords, tokens, addon configuration in addon URLs, keys inside stream links, server names and
addresses, and search text are removed.
