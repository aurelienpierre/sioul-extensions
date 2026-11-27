# Sioul's extensions

> **Unofficial.** The extensions here are not made, endorsed or supported by the vendors of the services they talk to, nor by any school or school platform. They talk to those services as their community clients do, and may stop working whenever a service changes.

Why this is lawful — the right to study and interoperate with software you use, which terms of use cannot waive — is set out under European Union and French law in [`LEGAL.md`](LEGAL.md).

An extension brings data into [Sioul](https://github.com/aurelienpierre/sioul) from a service Sioul does not speak to itself. Each is a small Rust program compiled to one WebAssembly file, the same on every system; Sioul runs it in a sandbox of its own, gives it only what this repository's signed manifest declares for it (the hosts it may reach, whether it keeps a sign-in), and takes back two kinds of data, each only from an extension whose manifest says it brings it: read-only agendas, and sets of contacts, which Sioul adds to an address book of the person's choice. The interface is written down in Sioul's repository, `docs/extensions.md` (version 1).

| Folder | What it is |
|---|---|
| `sioul-extension/` | The library every extension uses: the messages of Sioul's interface (agendas, each event with its own categories and the person it is with, and sets of contacts, with the bounds Sioul counts), their way through the module's memory, the web request, the secret, the sign-in, an iCalendar writer, and a stand-in of Sioul for tests on a computer. |
| `skolengo/` | **Skolengo timetable**: each pupil's lessons from a school platform built on Skolengo (any of those its table lists, `skolengo/src/platforms.rs`), one read-only agenda per pupil, from a week back to four weeks ahead. |
| `planity/` | **Planity appointments**: a Planity Pro salon's appointments, from its calendar export and its agenda documents joined into one event per booking, one read-only agenda of busy time, from sixty days back to a hundred and twenty ahead; and the clients of those appointments, as contacts. |
| `manifest.toml` | What Sioul offers: each extension, what it brings, the hosts it talks to, whether it signs in, its file and that file's SHA-256. Signed by hand into `manifest.toml.sig`. |
| `LEGAL.md` | The legal basis under EU and French law for independent, read-only interoperability clients. |
| `tools/build.sh` | Builds each extension's file reproducibly and prints its SHA-256. |
| `.github/workflows/release.yml` | Builds an extension's file from its tag's sources and attaches it to that release. It signs nothing. |

## Skolengo timetable

- **The sign-in is the mobile application's own**, as every community client of Skolengo does it: OpenID Connect by authorization code on the platform's own sign-in service, with the application's client identifier, its secret and its return address `skoapp-prod://sign-in-callback`. Sioul shows the platform's page in a browser of its own (a window on a computer, a screen of its own on Android) and takes the return; you type your password on the platform's page, never in Sioul. The refresh token is kept in your device's keyring.
- **What it asks**: the account's pupils (a parent's children, or the pupil whose account it is), then each pupil's agenda fifteen days at a time; a day the server fails on keeps what Sioul had for it.
- **What it brings**: each lesson as an event with its subject, its teachers and its room; a cancelled lesson as a cancelled event; one agenda per pupil.
- **Tested against recorded answers only**, written from the field names the community clients read, with invented people and schools (`skolengo/tests/recorded/`). It has never been run against a real platform by its authors.

### Ported from

The Skolengo extension is a port to Rust of what these free software projects do, read at the commits given; the comments in `skolengo/src/` cite their files and lines.

| Project | Author | Licence | Commit | What was taken |
|---|---|---|---|---|
| [scolengo-api](https://github.com/maelgangloff/scolengo-api) | Maël Gangloff | GPL-3.0-or-later | `4241a20` | the gateway's address, the client's identifier, secret and return address, the headers of every request, the requests for the account and the agenda, the table of platforms and their discovery addresses (`README.md`) |
| [ha-skolengo](https://github.com/infernalK/ha-skolengo) | infernalK | MIT | `20dd40f` | the scopes asked, the code's exchange and the renewal (the new refresh token kept), the pupils of a parent's account, fifteen days a request, a range answering 500 halved until the bad day is alone, the flattening of JSON:API documents |
| [jskolengo](https://github.com/vinceh121/jskolengo) | vinceh121 | GPL-3.0 | `b0f86e2` | consulted for the same constants |

Because it is a work based on GPL code, this repository is under the **GNU General Public License, version 3 or later** (`LICENSE`), as Sioul is. ha-skolengo's MIT code may be included in it; its copyright notice and the MIT licence's permission notice are in `LICENSES/ha-skolengo-MIT.txt`, as that licence asks.

## Planity appointments

- **Two sources, one event per booking.** Planity Pro publishes each calendar as an iCalendar export (a keyed web address in its settings): it gives the times, the services, the prices, the cancellations and the recurring bookings, but no client's name nor phone. The salon's agenda documents, read over Firebase, give the client's name and phone, the note, the all-day blocks and the bookings older than the export's window, but no service's name. The extension reads both and joins them on Planity's own booking key, so each booking is one event whichever source holds it, with the same identifier from one refresh to the next. When the two disagree on a time or a cancellation, the more recently updated record wins.
- **The sign-in is the salon's email and password, and the calendar's iCal private link** (in Planity Pro: the Admin tab, then « Paramètres Agenda » in the list on the left, « Gestion des agendas », then the link « Synchroniser avec partenaires » at the top; the manifest gives Sioul's form that label and that help under the field). Planity Pro is a Firebase web application; it signs in with an email and a password, not an OAuth redirect. Sioul collects the three in a calm form of its own (the extension only names the fields it needs); the extension checks that the address is Planity's calendar export of the same salon, does Firebase's own sign-in dance to a Pro-claimed token, and keeps in your device's keyring only the resulting refresh token, the salon's business id and the calendar's address — never the password, neither in Sioul nor in the extension. The address and its key never reach the log. The Firebase API key in the source is public (embedded in Planity's own web application).
- **What it reads**: the calendar's export, and the salon's appointment documents (`vevents`) over the Firestore REST API, for a window from sixty days back to a hundred and twenty ahead, paged past Firestore's page size. It reads nothing else; the rest of the salon's data is permission-denied to it.
- **What it brings**: each booking as one event of **busy** time, titled by the client and the services ("client — services"), a block or closure by its own words or as "Unavailable"/« Indisponible », an all-day block as a whole-day event, a cancelled booking as a cancelled event; each service with its duration, the total price, whether it was prepaid or booked online, the phone and the note in the description. A recurring booking becomes one event per occurrence within the window (daily or weekly rules, with their interval, days, end and exceptions); a recurring booking cancelled as a whole brings none. One read-only agenda per salon calendar, into a calendar you choose. Each event also carries its services as categories of its own (Sioul writes them after "Planity", on one line: "Planity, Coupe, Couleur"; a block's words are no category), and, for a named client, the client it is with, by the id the set of clients holds them by: Sioul ties the appointment to that client's card, in the calendar itself (`CONTACT;ALTREP="urn:uuid:<the card's UID>"`, never a guest), and never writes the id (0.2.1).
- **What it brings to an address book**: the clients of the appointments it read, as one set of contacts named "Clients", which Sioul writes into an address book you choose. These are the clients seen in the appointment documents of the window read, a cancelled appointment's included, not yet the salon's whole client list. Each client is one person, by Planity's own client id (which Sioul keeps only as a hash, never shows): the name as Planity writes it, from the most recently updated appointment (never split into a first and a last name), and the phone numbers and email addresses seen, the latest first, eight of each at most. A block, or a client without a name, brings nobody. Sioul only adds to the address book: it binds a client to a card you already have by a phone number or an email address (never by a name alone), adds at most what the card lacks, and never removes a number nor deletes a card. When only the calendar's export answered, no contacts come: it holds no client.
- **What failed, said precisely** (0.2.1). Every problem names its step (the form, the password sign-in, the Pro-token exchange, the custom-token sign-in, the token's renewal, Firestore, the feed, the keyring), the HTTP status, the service's own error code when it gives one (`INVALID_LOGIN_CREDENTIALS`, `TOO_MANY_ATTEMPTS_TRY_LATER`, `TOKEN_EXPIRED`, `INVALID_REFRESH_TOKEN`, `USER_DISABLED`, a key refused: `API_KEY_INVALID`), never the service's prose; which field of the form was empty or malformed, never its value; and whether no sign-in is kept on this device, the one kept does not read, or the keyring did not answer. Sioul shows that detail under its sentence, and the log has one line a step, every value hidden. The password is sent exactly as typed; the email and the link lose the spaces a phone's keyboard adds around them.
- **Degraded, never destructive.** When one source fails, the other's bookings still come, and nothing is removed from your calendar until both answer again. A sign-in that ended, or an address that is refused, asks you to sign in again.
- **Tested against recorded answers only**, written from the request and response shapes, with an invented salon, invented clients, fake tokens and a fake address key (`planity/tests/recorded/`). It has never been run against the live service by its authors; the coordinator validates that separately.

## Building and testing

```sh
cargo test                      # the library and each extension, on a computer, against recorded answers
tools/build.sh                  # every extension's file in out/, with its SHA-256
tools/build.sh skolengo         # one
```

`tools/build.sh` needs Rust with the WebAssembly target (`rustup target add wasm32-unknown-unknown`). The same sources, `Cargo.lock` and Rust version make the same bytes: paths are remapped, the build is one unit with no debug information. The automation uses Rust 1.99.0 (`RUST` in the workflow); a hash holds for that version.

## Making a release

Nothing is signed by the automation, and it holds no key. Each step below is done by the publisher.

1. **Change the extension**, raise its version in its `Cargo.toml`, run `cargo test`, and commit on a branch off `main`, with its manifest entry (see "The order" below).
2. **Tag it** `<id>-<version>` (for instance `skolengo-0.1.1`) and push the tag. The workflow builds the file from the tag's sources with the fixed Rust, runs the tests first, attaches `<id>-<version>.wasm` and `SHA256SUMS` to the release of that tag, and prints the hashes in its log. (Turning on GitHub's release immutability keeps an attached file from being replaced later.)
3. **Build the same tag on your own computer: this step is required.** Check out the tag, read what changed since the last release, and run `tools/build.sh <id>` with the same Rust (`rustc --version` must say the workflow's `RUST`). Compare the SHA-256 it prints with the one in the release's `SHA256SUMS`. **Sign only when the two are equal.** The hash you sign is the one your own build printed, from the commit you read; the automation's file is offered only because it is the same bytes. When they differ, sign nothing: another Rust, another Cargo configuration (`~/.cargo/config.toml`, `CARGO_ENCODED_RUSTFLAGS`) or a changed file explains it, or the automation built something else.
4. **Write the manifest**: in `manifest.toml`, the extension's `version`, its `file` (the release's address of the file) and its `sha256`, the hash of step 3; raise `serial` by one (Sioul refuses a manifest whose serial is lower than the last it accepted, so that an older one cannot be served again). A new host or a new sign-in is asked of each person before the update is taken.
5. **Sign the manifest by hand**, on your computer, with the security key that holds the publisher's key plugged in:

   ```sh
   gpg --no-textmode --local-user '2EF12D28F49E347CAD7E6BF31549D8B45963166A!' --digest-algo SHA512 --detach-sign --output manifest.toml.sig manifest.toml
   ```

   The `!` makes GnuPG sign with that primary key itself rather than a signing subkey: Sioul holds that key's packet alone and checks that the signature's issuer is it, that it is a signature of a binary document, and that it holds over the manifest's exact bytes. Do not change `manifest.toml` after signing it, not even a line ending.

   **At each signing**, even of a manifest that changes nothing but its serial, check again that every file it names is still the one its `sha256` names: download each from its release and compare (`sha256sum`). A release's files can be replaced on GitHub unless its release immutability is on; Sioul then refuses the replaced file, and no one can install that extension until it is put back.
6. **Publish**: commit `manifest.toml` and `manifest.toml.sig` together and push them to `main`. Sioul reads them from `https://raw.githubusercontent.com/aurelienpierre/sioul-extensions/main/manifest.toml` (and `.sig` beside it) when its Extensions page opens, and once a day while an extension is enabled. Then, at Sioul's next release, that serial becomes the lowest Sioul accepts (`LOWEST_SERIAL`, in Sioul's `crates/sioul-core/src/extensions/mod.rs`), so that a device that never saw it, a new phone for instance, cannot be served an older manifest. A manifest signed for a trial takes a serial below the next one published.

**The order** (learnt when the first `planity-0.2.0` tag failed): an extension's tests read its own entry in `manifest.toml`, so the new entry of step 4 must already be in the commit you tag; and a `manifest.toml` changed on `main` without its matching signature makes every Sioul refuse the whole list. So: commit the extension and its manifest entry on a branch off `main`, the hash being that of your own build of that commit; tag that commit, signed, and push the tag alone; let the workflow attach the file and compare its hash with yours; sign the manifest; then bring the branch, the manifest and its `.sig` to `main` together and push.

Before the repository is public, Sioul can read a manifest from a folder of your computer instead (`[extensions] manifest = "/path/to/manifest.toml"` in Sioul's configuration), its signature checked all the same; a file named by a relative `file` is then read beside it.
