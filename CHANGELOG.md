# Changelog

All notable changes to Horcrux are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/mareekkk/horcrux/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/mareekkk/horcrux/releases/tag/v0.1.0
