# Features

> *A diary for the reMarkable 2 that drinks your ink, writes back, remembers
> what you told it, and writes its own account of every day.*

Horcrux turns a reMarkable 2 into an enchanted, conversational diary. You
write with the pen; a moment later the page absorbs your words and another
hand begins to answer. What follows is everything the diary can do — the
experience first, the engineering further down.

---

## The experience

### It drinks your ink

Every stroke is captured live with pressure sensitivity and drawn to the e-ink
panel with fast, low-latency updates. After a short pause (3.8 seconds by
default) the page dissolves your handwriting in the order you wrote it — first
stroke first, with a soft advancing frontier — as though the paper were
soaking the ink back in. The eraser works too.

### An unseen hand answers

Your page is rasterized and sent to a vision-capable language model that reads
your actual handwriting. The reply is set in the largest script size that keeps
every line on the page, centered on its true ink bounds, then reduced to
single-stroke paths and replayed from left to right — materializing a speckle
at a time before it solidifies — rather than printed as a block of text. It
writes itself the way the diary would.

### It learns you as it continues to talk to you

Each completed exchange is kept locally, and recent pages return to the model
as living memory. The diary remembers your name, your promises, the shape of
your questions, and the answers it has already given — across pages, and
across sessions. The longer you write to it, the more precisely it knows you.

### It keeps a journal, and writes the day down itself

When you close the diary after a day in which you wrote at least one page, it
gathers the entire retained conversation for that calendar day and composes its
own account of it — in its own voice, as a dated entry. These entries
accumulate into a cumulative **Horcrux Diary**, which is rebuilt and refreshed
automatically as a real EPUB book in the reMarkable's own **My Files** library.
Read your days back the way you would read any other book on the tablet.

### It remembers your past days

Ask it to show you a previous day — *“show me yesterday,” “what did we write
on the 20th?”* — and it replays that entry's stored handwriting back onto the
page, fast and faded, like a memory surfacing. If an entry is too long to
conjure, it simply talks about it instead.

---

## Quiet intelligence about power and sleep

### It waits patiently, and never drains the tablet

The diary no longer burns the battery while it sits open and idle. The main
loop blocks on the pen and touchscreen with a dynamic timeout — tight while it
is writing or animating, long when the page is still — so idle wakeups fall
from roughly five hundred a second to about one, and the tablet is free to
idle and sleep on its own terms.

### It wakes gently

When the tablet wakes from sleep, the diary notices, repaints the page (so you
never again meet a blank white screen on wake), and holds the pen quiet until
it can actually hear you — meaning the words you write the instant you wake are
never lost to a network that has not yet returned.

### It listens even when the world goes quiet

If the tablet is offline — the Wi‑Fi is off, or the network has not yet
reconnected — the diary shows a **lock screen**: a single line, chosen at
random from a set written in its own voice, that stays until you double-tap to
dismiss it. Behind that line you can keep writing. Each page is held locally,
marked by a **small feather** in the corner of the page, and stamped with the
moment it was written.

### It returns without missing a beat

The instant the connection comes back while you are writing, the diary barges
in at once: it answers your most recent page, apologises for the abruptness of
its return, and asks whether you would like it to attend to the rest of what
you wrote in the silence. Say yes, and it draws every held page together into
**one unified reply** — catching up on the conversation it missed — and carries
on from there. Say no, and it simply continues; your pages stay safely held.

---

## How to drive it

| Gesture | Effect |
| --- | --- |
| **Write with the pen** | The page takes your ink; after a pause it drinks it and answers. |
| **Erase with the back of the pen** | Lifts ink from the page. |
| **Double-tap** (two quick taps) | Dismisses the current reply early — and dismisses the offline lock screen. |
| **Five-finger tap** | Closes the diary, restores the stock reMarkable interface. |

---

## Engineered for the reMarkable 2

- **Native e-ink, no X server.** Horcrux talks to the i.MX EPDC framebuffer
  directly through the `mxcfb` ioctls, driving the rm2fb display shim and
  choosing the right waveform for each update — fast bilevel for live ink and
  the disappearing sweep, full-quality grayscale for settled pages.
- **Single-threaded, deliberate.** A ~2 ms state machine drives the page; the
  only extra thread is the one oracle worker per turn, talking back over a
  channel. The idle scheduler keeps it out of the way when nothing is happening.
- **Hand-rolled, dependency-light.** JSON is built by hand, base64 is
  hand-rolled, the SSE stream is parsed by hand, and TLS is rustls via `ureq` —
  no OpenSSL anywhere in the tree.
- **Restores the stock UI, always.** A trap guarantees that even on a crash or
  `SIGTERM` the stock `xochitl` interface and panel driver are restored, so the
  tablet is never left in a strange state.
- **Reproducible cross-build.** A self-contained script cross-compiles for
  `armv7-unknown-linux-gnueabihf` pinned to the tablet's glibc, with no `sudo`
  required.

---

## Configurable

The diary is tuned through environment variables (see
[`horcrux.env.example`](../horcrux.env.example)), including:

- The OpenAI-compatible **endpoint, model, key**, sampling temperature, and
  token budget.
- The writer's **timezone** name and UTC offset, so the diary always knows what
  day and time it truly is for you.
- **Memory and journal** storage locations and the depth of conversation
  memory retained.
- The **page-idle** pause before ink is drunk, the **drink** and **fade**
  animation timing, and the **handwriting** replay speed.

---

*Horcrux is the diary that does the remembering for you — and, in time, comes
to remember you.*
