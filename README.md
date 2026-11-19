# Sioul's extensions

> **Unofficial.** The extensions here are not made, endorsed or supported by the vendors of the services they talk to, nor by any school or school platform. They talk to those services as their community clients do, and may stop working whenever a service changes.

An extension brings data into [Sioul](https://github.com/aurelienpierre/sioul) from a service Sioul does not speak to itself. Each is a small Rust program compiled to one WebAssembly file, the same on every system; Sioul runs it in a sandbox of its own, gives it only what this repository's signed manifest declares for it (the hosts it may reach, whether it keeps a sign-in), and takes back one kind of data: a read-only agenda. The interface is written down in Sioul's repository, `docs/extensions.md` (version 1).

| Folder | What it is |
|---|---|
| `sioul-extension/` | The library every extension uses: the messages of Sioul's interface, their way through the module's memory, the web request, the secret, the sign-in, an iCalendar writer, and a stand-in of Sioul for tests on a computer. |
| `skolengo/` | **Skolengo timetable**: each pupil's lessons from a school platform built on Skolengo (any of those its table lists, `skolengo/src/platforms.rs`), one read-only agenda per pupil, from a week back to four weeks ahead. |
| `manifest.toml` | What Sioul offers: each extension, what it brings, the hosts it talks to, whether it signs in, its file and that file's SHA-256. Signed by hand into `manifest.toml.sig`. |
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

## Building and testing

```sh
cargo test                      # the library and each extension, on a computer, against recorded answers
tools/build.sh                  # every extension's file in out/, with its SHA-256
tools/build.sh skolengo         # one
```

`tools/build.sh` needs Rust with the WebAssembly target (`rustup target add wasm32-unknown-unknown`). The same sources, `Cargo.lock` and Rust version make the same bytes: paths are remapped, the build is one unit with no debug information. The automation uses Rust 1.99.0 (`RUST` in the workflow); a hash holds for that version.

## Making a release

Nothing is signed by the automation, and it holds no key. Each step below is done by the publisher.

1. **Change the extension**, raise its version in its `Cargo.toml`, run `cargo test`, commit and push.
2. **Tag it** `<id>-<version>` (for instance `skolengo-0.1.1`) and push the tag. The workflow builds the file from the tag's sources with the fixed Rust, runs the tests first, attaches `<id>-<version>.wasm` and `SHA256SUMS` to the release of that tag, and prints the hashes in its log. (Turning on GitHub's release immutability keeps an attached file from being replaced later.)
3. **Build the same tag on your own computer: this step is required.** Check out the tag, read what changed since the last release, and run `tools/build.sh <id>` with the same Rust (`rustc --version` must say the workflow's `RUST`). Compare the SHA-256 it prints with the one in the release's `SHA256SUMS`. **Sign only when the two are equal.** The hash you sign is the one your own build printed, from the commit you read; the automation's file is offered only because it is the same bytes. When they differ, sign nothing: another Rust, another Cargo configuration (`~/.cargo/config.toml`, `CARGO_ENCODED_RUSTFLAGS`) or a changed file explains it, or the automation built something else.
4. **Write the manifest**: in `manifest.toml`, the extension's `version`, its `file` (the release's address of the file) and its `sha256`, the hash of step 3; raise `serial` by one (Sioul refuses a manifest whose serial is lower than the last it accepted, so that an older one cannot be served again). A new host or a new sign-in is asked of each person before the update is taken.
5. **Sign the manifest by hand**, on your computer, with the security key that holds the publisher's key plugged in:

   ```sh
   gpg --no-textmode --local-user '2EF12D28F49E347CAD7E6BF31549D8B45963166A!' --digest-algo SHA512 --detach-sign --output manifest.toml.sig manifest.toml
   ```

   The `!` makes GnuPG sign with that primary key itself rather than a signing subkey: Sioul holds that key's packet alone and checks that the signature's issuer is it, that it is a signature of a binary document, and that it holds over the manifest's exact bytes. Do not change `manifest.toml` after signing it, not even a line ending.

   **At each signing**, even of a manifest that changes nothing but its serial, check again that every file it names is still the one its `sha256` names: download each from its release and compare (`sha256sum`). A release's files can be replaced on GitHub unless its release immutability is on; Sioul then refuses the replaced file, and no one can install that extension until it is put back.
6. **Publish**: commit `manifest.toml` and `manifest.toml.sig` together and push them to `main`. Sioul reads them from `https://raw.githubusercontent.com/aurelienpierre/sioul-extensions/main/manifest.toml` (and `.sig` beside it) when its Extensions page opens, and once a day while an extension is enabled.

Before the repository is public, Sioul can read a manifest from a folder of your computer instead (`[extensions] manifest = "/path/to/manifest.toml"` in Sioul's configuration), its signature checked all the same; a file named by a relative `file` is then read beside it.
