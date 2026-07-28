# Changelog

All notable changes to Horcrux are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.1] - 2026-07-28

### Added

- A cover for the **Horcrux Diary** EPUB: ink-on-parchment art rendered at
  build time — the title in the diary's own handwriting, a quill and inkwell,
  a hand-drawn frame, and aged paper — so the book shows a proper cover in the
  reMarkable library instead of a blank thumbnail.
- `horcrux --render-cover <path>` to render the cover PNG standalone.

## [0.3.0] - 2026-07-28

### Added

- Battery-saving idle scheduler: the main loop blocks on the input devices
  with a dynamic timeout instead of busy-polling every 2 ms, dropping idle
  wakeups from ~500/s to ~1/s so the tablet can idle and autosuspend.
- Wake detection: the diary notices resume from sleep (CLOCK_BOOTTIME against
  CLOCK_MONOTONIC), repaints the panel, and gates the pen until the oracle is
  reachable — so nothing written the instant the tablet wakes is lost.
- Offline lock screen: when the diary is open but the oracle is unreachable, a
  random inspirational line is shown and stays until the writer double-taps to
  dismiss it. No "connecting" spinner, and no separate offline notice — the
  line itself is the signal.
- Offline writing: with the network away the writer can still write. Pages are
  held locally, stamped with the moment they were written, and marked by a
  small feather in the corner of the page.
- Barging-in reconnect: the instant connectivity returns mid-conversation, the
  diary replies at once to the last page — apologising for the interruption —
  and asks whether to address the rest. A yes produces one unified summary of
  every held page; a no carries on, the pages left safely held.
- A reachability probe that can never freeze the interface: a hung DNS resolver
  on a dead network no longer blocks the main loop or stalls shutdown.

### Changed

- The stock-library EPUB is refreshed at session end whenever any entries
  exist, not only when the current day's composition succeeded.

### Fixed

- Battery drain while the diary sat idle with the page open.
- A blank or white screen appearing on wake from sleep.
- Offline-written pages being lost when written before the tablet reconnected.

## [0.2.0] - 2026-07-27

### Added

- Writer-local date and time context on every ordinary reply and journal turn,
  configurable through timezone name and UTC offset settings.
- A cumulative `Horcrux Diary.md` rebuilt from dated summaries whenever a
  session ends.
- A matching, automatically refreshed EPUB registered as **Horcrux Diary** in
  the stock reMarkable My Files interface.
- `horcrux --publish-diary` for rebuilding the Markdown and library document
  without starting the display application.

### Changed

- Refined the diary voice toward restrained, poetic, otherworldly mystery.
- Prevented replies from addressing the writer by name unless the writer
  explicitly asks for their name to be used in that reply.
- Prevented the diary from volunteering its own identity unless directly
  asked.
- Reduced reply brush weight and set completed replies to linger for four
  seconds before fading.
- Regenerated the current day's summary at every session end so it incorporates
  all newly written pages rather than keeping the first summary of the day.

### Fixed

- Prevented long three-sentence replies from disappearing below the screen:
  writing now waits for the complete response, automatically selects the
  largest font size that fits, and centers the actual ink bounds before the
  first stroke appears.
- Added a 55-word hard limit for ordinary replies when a model ignores the
  prompt's response-length instruction.
- Made deployment compatible with the tablet's BusyBox userspace, which does
  not provide the GNU `install` command.

## [0.1.0] - 2026-07-27

### Added

- Pressure-sensitive pen capture and eraser support on the reMarkable 2.
- Fast rm2fb-backed e-ink drawing and ordered disappearing-ink animation.
- OpenAI-compatible vision requests with streamed, in-character replies.
- Script-font rasterization, skeletonization, stroke tracing, and animated
  left-to-right handwriting replay.
- Persistent local conversation memory with configurable context depth.
- Automatic daily journal composition from the full retained conversation for
  that calendar day.
- Dated journal storage as readable text and replayable handwriting strokes.
- Natural-language recall and faded replay of past journal entries.
- Five-finger exit with reliable restoration of the stock reMarkable UI.
- Optional xovi/AppLoad integration that survives tablet reboots.
- Environment controls for page-idle timing, disappearing ink, handwriting
  speed, reply fading, and explicit memory/journal storage locations.
- Version reporting through `horcrux --version` and versioned startup logs.
- Reproducible ARMv7 cross-build and release packaging scripts.

[Unreleased]: https://github.com/mareekkk/horcrux/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/mareekkk/horcrux/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/mareekkk/horcrux/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/mareekkk/horcrux/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/mareekkk/horcrux/releases/tag/v0.1.0
