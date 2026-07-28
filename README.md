<p align="center">
  <img src="packaging/appload/icon.png" width="160" alt="Tom's Diary icon">
</p>

<h1 align="center">Horcrux</h1>

<p align="center">
  <em>A diary for the reMarkable 2 that drinks your ink, writes back,<br>
  remembers what you told it, and writes its own account of every day.</em>
</p>

<p align="center">
  <a href="https://github.com/mareekkk/horcrux/actions/workflows/ci.yml"><img src="https://github.com/mareekkk/horcrux/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/mareekkk/horcrux/releases/latest"><img src="https://img.shields.io/github/v/release/mareekkk/horcrux?display_name=tag" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-7a5a34" alt="MIT license"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-2021-6b4c2f?logo=rust" alt="Rust 2021"></a>
</p>

---

Write a question on the page. A few seconds after the pen leaves the paper,
the diary consumes the handwriting from left to right. The page goes still.
Then another hand begins to write.

Horcrux turns a reMarkable 2 into an enchanted, conversational diary. It
captures real pen strokes, sends the page to a vision-capable language model,
and renders the answer as animated handwriting directly on the e-ink display.
It remembers earlier pages, keeps a dated journal, and returns cleanly to the
stock reMarkable interface when closed.

> **The diary does not merely remember the day. It writes the day down
> itself.**

➜ For the full tour, see **[Features](docs/FEATURES.md)**.

## The enchantment

### The page drinks your ink

Pen input is rendered immediately with pressure-sensitive strokes and fast
e-ink updates. After a configurable pause (3.8 seconds by default), the page
dissolves the writer's ink in writing order, as though the paper were
absorbing it.

### An unseen hand answers

The written page is rasterized and sent to an OpenAI-compatible vision model.
The complete reply is set in the largest script size that keeps every wrapped
line visible, then its actual ink bounds are centered on the screen before the
first stroke appears. It is reduced to single-stroke paths and replayed from
left to right rather than displayed as a block of text.

### The diary remembers

Every completed exchange is stored locally. Recent pages return to the model
as conversational memory, allowing the diary to remember names, promises,
questions, and earlier answers across sessions.

### At day's end, the diary writes its own entry

When the diary is closed after a day in which at least one page was written,
it gathers the **entire retained conversation from that calendar day**—every
question and every answer—and composes a short first-person account in Tom's
voice.

That daily summary then writes itself onto a clean page before the diary
closes. Both its words and its handwriting strokes are saved under that date.
Only one entry is created per day. A second five-finger tap during composition
skips the ritual and closes immediately.

This is more than a chat history: the diary becomes an author of its own
memory.

### Old pages can be conjured

Ask naturally:

- “Show me yesterday's entry.”
- “What did we write on the 20th?”
- “Let me see the entry from 4 July.”

The stored handwriting is replayed quickly in faded ink. If an entry is too
large to conjure safely, the diary recalls its opening sentences instead.

### It leaves no visible machinery behind

A five-finger tap closes Horcrux, stops its display server, and restores the
normal reMarkable UI. With the optional AppLoad integration, **Tom's Diary**
appears as an on-tablet application and returns automatically after a reboot.
No laptop or USB cable is needed after installation.

## Feature list

- Live pressure-sensitive pen input from the reMarkable digitizer
- Fast partial e-ink refreshes through rm2fb
- Left-to-right disappearing-ink animation
- Vision-model reading of natural handwriting
- Streaming replies that begin writing before the model has finished
- Script-font rasterization, thinning, stroke tracing, and animated replay
- Persistent conversational memory with configurable context depth
- Automatic once-per-day summary of the full retained daily conversation
- Dated journal storage as both readable text and replayable handwriting
- Natural-language recall of earlier journal dates
- Eraser support
- Five-finger exit and reliable restoration of the stock UI
- Optional persistent AppLoad launcher for cable-free use
- Configurable model endpoint, model, font, memory, journal, and timezone
- No cloud database: memory and journal files remain on the tablet

## Compatibility

Horcrux currently targets:

- **Device:** reMarkable 2
- **Architecture:** 32-bit ARMv7 hard-float
- **Tested OS:** reMarkable OS 3.22.4.2
- **Display backend:** rm2display/rm2fb from
  [timower/rM2-stuff](https://github.com/timower/rM2-stuff)

The reMarkable 1, Paper Pro, and Paper Pro Move are not supported.

> [!CAUTION]
> rm2display and xovi are tied to particular reMarkable OS releases. Confirm
> compatibility before updating the tablet. An unsupported OS update may
> remove or break the display shim and launcher.

## Requirements

- A reMarkable 2 with SSH access
- A compatible rm2display installation providing
  `/opt/lib/librm2fb_client.so.1`
- Wi-Fi on the tablet
- An OpenAI-compatible endpoint with a vision-capable model
- An API key for that endpoint
- For the optional launcher:
  [xovi](https://github.com/asivery/xovi) and AppLoad builds compatible with
  the installed reMarkable OS

## Install a release

Download the latest reMarkable 2 archive and its checksum from
[GitHub Releases](https://github.com/mareekkk/horcrux/releases/latest), verify
it, and extract it:

```sh
sha256sum -c horcrux-v*-remarkable2-armv7.tar.gz.sha256
tar -xzf horcrux-v*-remarkable2-armv7.tar.gz
cd horcrux-v*-remarkable2-armv7
scripts/deploy.sh
```

The default device address is `root@10.11.99.1` over USB. A Wi-Fi address can
be passed explicitly:

```sh
scripts/deploy.sh root@192.168.20.45
```

Deployment copies the binary, launchers, fonts, and systemd unit to
`/home/root/horcrux/`. It never overwrites an existing private configuration.

## Build from source

The one-command cross-build downloads a pinned Zig toolchain locally and
targets the glibc version used by supported reMarkable OS releases:

```sh
scripts/build.sh
scripts/deploy.sh
```

See [BUILD.md](BUILD.md) for the compiler, ABI, and dynamic-linking details.

## Configure the oracle

On the tablet:

```sh
cp /home/root/horcrux/horcrux.env.example /home/root/horcrux/horcrux.env
chmod 600 /home/root/horcrux/horcrux.env
$EDITOR /home/root/horcrux/horcrux.env
```

Set at least:

```sh
HORCRUX_OPENAI_KEY=CHANGEME
```

The configuration file is excluded from Git and should always remain mode
`600`.

## Run

Start from SSH:

```sh
systemctl start horcrux.service
```

Or install the on-tablet launcher after compatible xovi and AppLoad builds are
already working:

```sh
scripts/install-appload.sh
```

Reboot once, then open **AppLoad → Tom's Diary**. The diary unit intentionally
remains disabled at boot; the stock UI starts normally and the launcher opens
the diary only when selected.

### Gestures

| Gesture | Effect |
| --- | --- |
| Write, then lift the pen | Submit after the configured idle pause |
| Use the eraser | Remove nearby ink |
| Double tap | Dismiss a completed reply sooner |
| Five-finger tap | Write today's entry, then exit |
| Five-finger tap again while journaling | Skip the entry and exit immediately |

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `HORCRUX_OPENAI_KEY` | — | **Required.** API key for the model endpoint |
| `HORCRUX_OPENAI_BASE` | `https://api.openai.com/v1` | OpenAI-compatible base URL |
| `HORCRUX_OPENAI_MODEL` | `gpt-4o-mini` | Vision-capable model |
| `HORCRUX_OPENAI_MAX_TOKENS` | `2000` | Reply length cap |
| `HORCRUX_OPENAI_TEMPERATURE` | `0.9` | Sampling temperature |
| `HORCRUX_REASONING_SPLIT` | auto | Separate reasoning for compatible models |
| `HORCRUX_FONT` | embedded Dancing Script | Optional `.ttf` reply font |
| `HORCRUX_IDLE_MS` | `3800` | Quiet time before submitting a written page |
| `HORCRUX_DRINK_STAGE_MS` | `140` | Delay between disappearing-ink stages |
| `HORCRUX_WRITE_TICK_MS` | `16` | Delay between handwriting batches |
| `HORCRUX_WRITE_POINTS_PER_TICK` | `12` | Points written per animation batch |
| `HORCRUX_FADE_STAGE_MS` | `140` | Delay between reply-fade stages |
| `HORCRUX_MEMORY` | `on` | `off` disables persistent memory |
| `HORCRUX_MEMORY_DIR` | `/home/root/horcrux-data/memories` | Memory storage |
| `HORCRUX_MEMORY_TURNS` | `6` | Recent turns supplied as conversational context |
| `HORCRUX_JOURNAL` | `on` | `off` disables daily entries and recall |
| `HORCRUX_JOURNAL_DIR` | beside memory, under `journal/` | Daily text and stroke storage |
| `HORCRUX_LIBRARY_DIR` | reMarkable xochitl library | Destination for the readable diary EPUB |
| `HORCRUX_TZ_NAME` | `Australia/Perth` | Writer timezone name supplied to the oracle |
| `HORCRUX_TZ_OFFSET` | `8` | Whole hours added to UTC for the writer's local clock |

Animation values are deliberately bounded so an accidental zero or extreme
value cannot lock the display loop. Invalid values are ignored and the default
is used. See [horcrux.env.example](horcrux.env.example) for ranges, examples,
and detailed descriptions of every variable.

### On-tablet data

```text
/home/root/horcrux-data/
├── memories/
│   ├── index.tsv
│   └── <timestamp>.strokes
└── journal/
    ├── Horcrux Diary.md
    ├── YYYY-MM-DD.txt
    └── YYYY-MM-DD.strokes
```

Memory is capped at the 400 most recent conversational pages. The daily entry
uses every page from the selected day that remains in that retained history.
At every session end, the current day's summary is regenerated so it includes
the latest pages, and the cumulative Markdown diary is rebuilt. Because the
stock reMarkable interface does not open Markdown, Horcrux also refreshes a
matching **Horcrux Diary** EPUB in My Files. Journal entries do not feed back
into conversation memory.

## How the spell is made

Horcrux is a small Rust application with a single-threaded display/input state
machine:

```text
Listening → Drinking → Thinking → Replying → Lingering → Fading
                           ↘ Journaling
                           ↘ Conjuring
```

- `pen.rs` reads raw evdev pen and touch input.
- `surface.rs` writes RGB565 pixels through the rm2fb shared framebuffer.
- `ink.rs` records strokes, builds the model image, and plans dissolves.
- `oracle.rs` streams an OpenAI-compatible vision response.
- `script.rs` rasterizes, thins, traces, and orders reply strokes.
- `memory.rs` stores conversation pages locally.
- `journal.rs` composes, stores, dates, and recalls daily entries.
- `main.rs` conducts the state machine and animations.

## Privacy and security

- The page image and configured conversational context are sent to the chosen
  model endpoint.
- API credentials remain in the on-tablet `horcrux.env`.
- Conversation memory and journal entries are plain local files.
- No telemetry, analytics, account system, or hosted Horcrux service exists.
- Before sharing logs, remove personal writing, endpoint details, and any
  credentials.

Please report security problems privately as described in
[SECURITY.md](SECURITY.md).

## Development

```sh
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo run --example render_sample
```

The project follows Semantic Versioning and keeps a human-readable
[changelog](CHANGELOG.md). Release maintainers should follow
[the release guide](docs/RELEASING.md).

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) and the
[Code of Conduct](CODE_OF_CONDUCT.md) before opening a pull request.

## Recovery

Making xovi boot-persistent is convenient, but it removes part of xovi's
tethered recovery safety. If an extension ever causes a boot loop, connect
over USB and run:

```sh
systemctl disable xovi.service
reboot
```

Horcrux itself restores `xochitl` through an exit trap, including after most
application failures. Logs are available from:

```sh
journalctl -u horcrux.service
cat /tmp/horcrux.log
```

## Credits

- Inspired by [riddle](https://github.com/MaximeRivest/riddle) by Maxime
  Rivest (MIT), reimplemented in Rust for the reMarkable 2.
- [Dancing Script](https://fonts.google.com/specimen/Dancing+Script) by Pablo
  Impallari and Cedarville Cursive by Kimberly Geswein, distributed under the
  SIL Open Font License 1.1. License texts are included in `fonts/`.
- Display definitions were cross-checked against
  [FBInk](https://github.com/NiLuJe/FBInk) and
  [rmkit](https://github.com/rmkit-dev/rmkit).

See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for details.

## Disclaimer

Horcrux is an unofficial fan project. It is not affiliated with or endorsed
by reMarkable AS, J.K. Rowling, Warner Bros., or their affiliates. Harry
Potter and related characters and elements are trademarks of their respective
owners. Running custom software on a tablet is at your own risk.

## License

The Horcrux source code and original project assets are available under the
[MIT License](LICENSE). Bundled fonts remain under the SIL Open Font License.
