# Myco v0.9.1

**Released**: 2026-10-02

v0.9.1 lets you **write**, not just read, and makes apps **open faster**.

- Composer, for writing notes, replies and quotes, and Noris, a reader for
  long-form articles and highlights, come with new installs.
- Apps open and fill about twice as fast.
- The download is a third of the size: about 22 MB.

**No wire-format change from v0.9.0.** v0.9.1 and v0.9.0 phones pair and
exchange apps and messages as before. Everything upgrades in place.

## At a glance

- **Composer comes with Myco** on new installs. Write a note, reply to one or
  quote one. A reply shows the note you're answering while you type, and
  everyone in the conversation is notified. Type `@` to mention someone you
  follow; `#words` become hashtags; Preview shows the note before it goes out.
  Other apps open it by role, so their Reply and Quote buttons land in it.
- **Noris comes with Myco** on new installs. Read long-form Nostr articles in
  a calm reader after Boris: highlights from you, people you follow and
  everyone else are painted in the text, you can highlight what you read, and
  Noris remembers where you stopped. Articles and highlights opened from
  Chronofeed land in it.
- **Apps open faster.** An app's data now streams in instead of arriving in
  one batch, and far less work is repeated on the way: on a Pixel 7 Pro,
  Noris's articles appear in about a quarter to a third of a second instead of
  two-thirds.
- **A smaller download.** The app is about 22 MB, down from 78 MB.

Existing installs keep their apps as they are; Composer and Noris can be
installed from the AppStore.

## Also

- **Tapping an app after Myco restarted opens it.** After an update, or when
  Android had closed Myco in the background, tapping an app could land you
  back in Myco instead.

## Known issues

- **Web pictures in apps.** Napplets load files by their hash (`blossom:`).
  Plain web images, including many profile pictures, show a placeholder (🥀 in
  Chronofeed). `https:` image support is still to come;
  [#67](https://github.com/Origami74/myco/issues/67) covers Blossom server
  discovery.
- **Few apps declare a role yet.** Opening "a profile", "an article" or "a
  composer" needs an app whose manifest declares that archetype, such as
  Simple Profile, Noris or Composer; other napplets don't yet. `article`,
  `highlight` and the `composer` convention are ours, not yet in the NAAT
  registry. Messages between apps other than opening by role (`inc.emit`)
  aren't routed yet.
- **Reading positions in Noris stay on the phone.** Boris publishes them as
  public events; Noris keeps them on the device instead, because Myco can't
  encrypt for an app yet (`relay.publishEncrypted`). Without app storage they
  last for the session.
- **Composer can't attach pictures yet.** Myco doesn't offer uploads to apps
  (NAP-UPLOAD); pasting a Blossom link works.
- **A custom relay is read, not streamed.** With a custom relay set, reads wait
  for it rather than streaming as they arrive.
- **Big files are held in memory** while they load (up to 64 MB each).
- **Paid relays.** Relays that require a paid account (nostr.wine, for one)
  refuse Myco's reads, so apps published only there can't be found.
- **Open nsite windows and updates.** An update can mix old and new files in
  an open nsite window until it reloads:
  [#71](https://github.com/Origami74/myco/issues/71).
- **No "recently updated" mark on the Apps screen yet:**
  [#69](https://github.com/Origami74/myco/issues/69).
