# Merlin

Read `AGENTS.md` first. It carries the architecture and the house rules
inherited from upstream and still applies in full. This file holds what is
specific to Merlin, and the decisions behind it, so they are not relitigated.

Merlin is a fork of [ZapFast](https://github.com/crmne/zapfast) by Carmine
Paolino, MIT. Keep the licence and the attribution in `README.md` intact.
Prefer sending general fixes upstream over letting the fork drift.

## Decisions already made

**Merlin installs beside ZapFast, it does not replace it.** The owner runs
ZapFast daily. Merlin keeps its own configuration, state, cache and keyring
entry, and links to the phone as its own device.

- `ADOPTED_NAMES` in `src/paths.rs` is empty on purpose and a test holds it
  that way. Adoption moves directories rather than copying them, so claiming
  ZapFast's would strip the archive and linked session from an installation
  still in use. The archive is the only copy of a user's history, because
  WhatsApp replays it once at link time.
- The single-instance guard in `src/single_instance.rs` uses port 47143 and a
  `merlin:` handshake. ZapFast and FastsApp share port 47119 and `fastsapp:`.
  They were identical before the rename, which made Merlin raise ZapFast's
  window and exit instead of starting. Never key either to a shared value.
- Identity lives under `rocks.merlin.Merlin` for the keyring, the macOS bundle
  and notifications, and the Windows application id. Never reuse
  `me.paolino.*`; that is the upstream author's namespace.

**A build from the source tree is a third app, not a second copy.** An
installed Merlin and `scripts/dev-run.sh` were the same program, so launching
one while the other ran only raised the first window. `src/profile.rs` reads
`MERLIN_PROFILE=dev` and switches the directory name, the instance port, the
wire prefix and the display name together, and `dev-run.sh` sets it. Anything
new that would collide between two running copies belongs in that one file,
beside the four that are already there.

## Interface

**There is no design system, and that is the root cause of most "this looks
unpolished" reports.** A count across `src/ui/` found 19 distinct font sizes
from 10.5 to 30, 14 explicit spacings including 3, 6, 10 and 18, four corner
radii and six vertical gaps. Every number was picked locally to look right in
its own widget. Nothing relates to anything, and the eye reads the missing
relationship as sloppiness even where no single element is wrong.

Until a scale exists, expect local fixes to keep surfacing new symptoms.
A proper pass would define a spacing scale in multiples of four, a type ramp
of about six sizes, two or three radii and one icon ramp, then replace every
literal. It needs a human eye on the result: the tests catch layout panics and
one bubble-width assertion, not aesthetics.

Already unified, do not scatter them again:

- `theme::PANE_INSET` is the one left and right inset for every pane's
  content. The macOS traffic-light offset measures from it.
- `theme::HEADER_ROW` and `theme::header_margin()` size both headers, so the
  two panes align across the divider.

**Measure before changing spacing.** Screenshots can be measured with Pillow:
find the bubble fill colour, find its bounds, then find rows carrying light
ink. Guessing at padding has been wrong more often than right here. Native
WhatsApp on iOS measures top 12.7, bottom 5.9, left 11.4, right 10.1 points;
Merlin is already more generous than that on every side.

The message time takes its own line under the text, lifted into the room the
last line leaves below its baseline. It is deliberately **not** inline, and
the width reservation that shortens the last line stays so it ends clear of
the bubble edge.

## Footprint

The project's claim is that it opens in under a second and idles near 150 MB.
Treat that as a budget.

**Copy `src/animation.rs`'s shape**: capped frame size and count, true
least-recently-used eviction, a twenty second drain after the last draw,
limited concurrent decoders, and an early return when a row is off screen.

**The cache ceilings are the budget, and they were once larger than the app's
own target.** A reading showed 184 MB of textures, 104 of them animation and
71 pictures, in an app aiming to idle near 150 MB. Animation now holds 180
frames rather than 450 and caps both sides of a frame, not only its width, so
a tall sticker no longer costs several times a wide one. Pictures hold about
32 MB. Raise either only against a measurement, never to make something look
sharper.

Fixed already, do not undo:

- The system emoji font is memory-mapped, not read. On macOS that file is
  188,589,668 bytes and the loader used to hold all of it on the heap for the
  process's life. The bundled fallback is borrowed from the binary, not copied.
- Staged picture files decode once at tile size.

Run with `--verbose` and the app writes a `memory:` line every half minute,
naming megabytes per cache. Use it before changing anything here; two rounds
of this were lost to reasoning from code instead.

The largest open item was image caching. Every drawn image is held three times,
as file bytes, as decoded pixels and as a texture. `reduce_texture_memory`
defaults to off and nothing calls the forget-image functions, and the texture
cache only evicts when one address has two size buckets, which never happens
for photos. `egui_extras`'s image loader discards the requested size, and
`picture()` in `src/ui/conversation.rs` calls it for every image in the open
conversation with no visibility check. Fix the visibility guard and the decode
size **before** enabling `reduce_texture_memory`: thumbnails register once
behind a deduplication set, so forgetting their bytes first would make them
unreloadable. The full list is in the owner's notes.

## Mistakes already made here

Each of these shipped or nearly shipped in this repository. They are cheap to
repeat and were expensive to find.

**Do not state what a platform API cannot do until you have listed its
surface.** "Quick Look only returns a first page" was true of the one function
in use and false of the framework. It nearly settled the whole preview design
on a wrong premise, and the owner caught it. Read the crate's generated
modules, or the framework's class list, before ruling an approach out.

**Never report a command as clean when you filtered its output.** A check
grepped for errors and `warning: unused` let every deprecation warning
through, and CI denies warnings. Run the gate the project runs, unfiltered,
and read the exit code.

**A test that passes on a machine lacking the feature is not testing it.**
Documents were asserted absent from the preview set, which was true on the
test machine only because it has no preview panel. Make the platform gate
forceable, then assert both branches.

**Measure memory, never infer it.** Two rounds were lost to reasoning about
allocation from source. The `memory:` line found the answer in one reading,
and the owner's before-and-after numbers localised it further. Ask for a
measurement before changing a cache.

**When two paths do the same job, join them or write down why not.** Pictures
went to the in-app viewer while documents went to the system panel, and they
drifted until a document between two photos broke the sequence. The same
split produced a poster cached as an animation.

**An edit that silently matches nothing is a shipped bug.** Moving code into a
module changed its indentation and a replacement quietly did nothing, leaving
a feature missing while every test stayed green. Assert the match, and check
the result compiles where it actually runs.

**Read an input source once per gesture, in one place.** One Ctrl+V reaches
the app twice, as a paste event on the press and a key release after it. The
clipboard was read at both, files at the first and pictures at the second, so
a clipboard holding a picture and its address staged the picture twice. Two
rounds of patching the second read failed because the shape was wrong. One
read, one decision, one record of having decided.

## Verifying macOS code

This environment cannot build the app for macOS; a dependency needs a
cross-compiler it lacks. The objc2 bindings are pure Rust, so platform code is
verified in a throwaway crate instead:

1. Make a small crate with only the objc2 dependencies.
2. Copy the exact `#[cfg(target_os = "macos")]` module into it.
3. Add `#![deny(warnings)]`.
4. Run `cargo clippy --target aarch64-apple-darwin -- -D warnings`.

Steps 3 and 4 are load-bearing. A plain `cargo check` filtered for errors let a
set of deprecation warnings through once, and CI denies warnings. Clippy also
catches what compilation accepts.

This verifies API calls and types. It cannot verify runtime behaviour.

## Distribution

Releases are cut by pushing a version tag. `cargo test --all-features`,
`cargo clippy --all-targets --all-features -- -D warnings` and
`cargo fmt --check` must pass first; CI runs them on all three platforms.

**Apple notarisation is an automated malware scan, not a content review.**
Apple does not judge what the app does. Merlin passes the things it does
check: the hardened runtime is on, and the only entitlement is microphone
access for voice messages. The **App Store** would be a content review and
would very likely reject an unofficial WhatsApp client, so do not go there.
Direct download and Homebrew avoid it entirely.

Known risks, none of which are notarisation problems:

- **WhatsApp's terms of service.** An unofficial client may breach them and
  can get an account suspended. This is the real risk and it falls on users.
  Keep the disclaimer in `README.md`.
- **Trademark.** Describing compatibility with WhatsApp is ordinary
  nominative use. Using their branding is not. The app icon inherited
  `#00a884`, WhatsApp's own brand green, which is worth changing to Merlin's
  own identity.
- **H.264 patents.** `openh264` compiles from source. Cisco's royalty-free
  offer covers their precompiled binaries, so building it does not inherit
  that cover. Rarely enforced against small free projects, but real.
- **Takedown risk.** Similar projects have attracted requests from Meta.

Publishing beyond the DMG needs repositories that do not exist yet. Homebrew
needs `harman314/homebrew-tap` plus a push token. Arch needs an AUR account
and SSH key. Neither is wired up.
