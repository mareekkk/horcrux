# horcrux

*Tom Riddle's diary, on a reMarkable 2.*

Write on the tablet with the pen. A few seconds after you stop, the diary
drinks your ink away and writes an answer back in animated handwriting.

Tested on a reMarkable 2 running reMarkable OS 3.22.4.2. Display shims and
xovi extensions are OS-version-sensitive; confirm their compatibility before
upgrading the tablet.

## What it does

- Draws live, pressure-sensitive pen ink with fast DU waveform updates.
- Dissolves the page from left to right after 2.8 seconds of idle pen time.
- Sends a downscaled image of the page to an OpenAI-compatible vision model.
- Skeletonizes the reply into pen strokes and writes it back left to right.
- Remembers recent conversations and maintains a recallable daily journal.
- Exits on a five-finger tap and restores the stock reMarkable UI.

## Requirements

- **reMarkable 2** (armv7l). The rM1, Paper Pro, and Paper Pro Move are not
  supported.
- **rm2display** from
  [timower/rM2-stuff](https://github.com/timower/rM2-stuff), including
  `/opt/lib/librm2fb_client.so.1`.
- An **OpenAI-compatible vision API** endpoint and key.
- Wi-Fi on the tablet for model requests.
- For the optional on-tablet launcher:
  [xovi](https://github.com/asivery/xovi) and a compatible AppLoad extension.

## Build and deploy

The build uses Zig to cross-compile a dynamically linked armv7 binary against
glibc 2.35. See [BUILD.md](BUILD.md) for the complete toolchain notes.

```sh
scripts/build.sh
scripts/deploy.sh                         # USB default: root@10.11.99.1
# scripts/deploy.sh root@192.168.20.45   # Wi-Fi works too
```

Deployment copies the binary, launchers, fonts, and systemd unit to
`/home/root/horcrux/`, then installs `horcrux.service`. It never overwrites an
existing `horcrux.env`.

Create the private configuration once on the tablet:

```sh
cp /home/root/horcrux/horcrux.env.example /home/root/horcrux/horcrux.env
chmod 600 /home/root/horcrux/horcrux.env
$EDITOR /home/root/horcrux/horcrux.env
```

Set `HORCRUX_OPENAI_KEY`, then start the diary:

```sh
systemctl start horcrux.service
```

The service stops `xochitl`, runs the app through the rm2fb client shim, and
always restores the stock UI when the app exits. Use a **five-finger tap** to
quit. Logs are available from `journalctl -u horcrux.service` and
`/tmp/horcrux.log`.

## Cable-free launcher and reboot

Once compatible xovi and AppLoad builds are installed and working, register
the included **Tom's Diary** entry and make xovi start at boot:

```sh
scripts/install-appload.sh                # USB default
# scripts/install-appload.sh root@192.168.20.45
```

Reboot once. AppLoad will then appear in the stock UI after every boot; open it
and tap **Tom's Diary**. The diary, API configuration, memory, journal, display
shim, and launcher all live on the tablet, so no USB cable or host computer is
needed after setup.

The installer deliberately does not bundle xovi or AppLoad, and it refuses to
run unless both are already present. Making tethered xovi boot-persistent
trades some of its recovery safety for convenience. If an extension ever
causes a boot loop, connect over USB and run:

```sh
systemctl disable xovi.service
reboot
```

The diary unit itself remains disabled at boot: the stock UI starts normally,
and AppLoad starts the diary only when asked.

## Configuration

`horcrux.env` is a shell environment file sourced by the launcher.

| Variable | Default | Meaning |
| --- | --- | --- |
| `HORCRUX_OPENAI_KEY` | — | **Required** API key |
| `HORCRUX_OPENAI_BASE` | `https://api.openai.com/v1` | OpenAI-compatible base URL |
| `HORCRUX_OPENAI_MODEL` | `gpt-4o-mini` | Vision-capable model |
| `HORCRUX_OPENAI_MAX_TOKENS` | `2000` | Reply length cap |
| `HORCRUX_OPENAI_TEMPERATURE` | `0.9` | Sampling temperature |
| `HORCRUX_REASONING_SPLIT` | auto | Separate reasoning for compatible models |
| `HORCRUX_FONT` | embedded Dancing Script | Optional `.ttf` reply font |
| `HORCRUX_MEMORY` | `on` | `off` disables long-term memory |
| `HORCRUX_MEMORY_DIR` | `/home/root/horcrux-data/memories` | Memory storage |
| `HORCRUX_MEMORY_TURNS` | `6` | Recent turns sent as context |
| `HORCRUX_JOURNAL` | `on` | `off` disables journal composition and recall |
| `HORCRUX_TZ_OFFSET` | `0` | Whole hours added to UTC for local dates |

Keep `horcrux.env` at mode `600`; it is excluded from Git. Conversation
memory and journal entries are plain files on the tablet.

## Journal

On a five-finger quit after a day with at least one conversation, the diary
composes a short first-person entry and writes it on screen before exiting.
A second five-finger tap skips composition. No duplicate entry is created for
the same day.

Ask to *show yesterday's entry* or *show the entry from the 20th* to replay a
stored page. Missing days receive an ordinary prose response, and recall turns
are not added to conversation memory.

Entries are stored beside the memory directory, normally under
`/home/root/horcrux-data/journal/`, as one `YYYY-MM-DD.txt` and
`YYYY-MM-DD.strokes` pair per day. Set `HORCRUX_TZ_OFFSET` if the tablet clock
runs in UTC.

## How it works

The main process is a single-threaded state machine:
`Listening → Drinking → Thinking → Replying → Lingering → FadingReply`.
One worker thread streams each model request; complete sentences can begin
writing before the whole reply arrives. Display I/O uses rm2fb's shared RGB565
buffer and i.MX EPDC update ioctls. Pen and touch input come directly from
evdev.

Host-side checks:

```sh
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo run --example render_sample
```

## Credits

- Inspired by [riddle](https://github.com/MaximeRivest/riddle) by Maxime
  Rivest (MIT), reimplemented in Rust for the reMarkable 2.
- [Dancing Script](https://fonts.google.com/specimen/Dancing+Script) by Pablo
  Impallari and Cedarville Cursive by Kimberly Geswein, both distributed
  under the SIL Open Font License 1.1. License texts are in `fonts/`.
- Display definitions were cross-checked against
  [FBInk](https://github.com/NiLuJe/FBInk) and
  [rmkit](https://github.com/rmkit-dev/rmkit).

## Disclaimer

This is an unofficial fan project. It is not affiliated with or endorsed by
reMarkable AS, J.K. Rowling, or Warner Bros. Harry Potter and related
characters and elements are trademarks of their respective owners. Running
custom software on a tablet is at your own risk.

## License

The source code is MIT licensed; see [LICENSE](LICENSE). Bundled fonts remain
under the SIL Open Font License.
