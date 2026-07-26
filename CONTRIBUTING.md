# Contributing to Horcrux

Thank you for helping improve the diary.

Horcrux interacts directly with an e-ink display, raw input devices, systemd,
and a remote model endpoint. Small changes can therefore have unusually visible
or device-specific consequences. Please keep changes focused and describe how
they were tested.

## Before opening an issue

- Search existing issues and discussions.
- Use the bug template for reproducible defects.
- Use GitHub's private vulnerability reporting for security problems.
- Never post API keys, private diary text, page images, or unredacted
  `horcrux.env` contents.

## Development setup

The host-side Rust checks do not require a tablet:

```sh
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo run --example render_sample
```

Cross-building for the reMarkable 2:

```sh
scripts/build.sh
```

See [BUILD.md](BUILD.md) for the target ABI and toolchain details.

## Pull requests

1. Open an issue first for large features or architectural changes.
2. Keep each pull request limited to one coherent change.
3. Add or update tests for changed behavior.
4. Update user documentation when configuration or behavior changes.
5. Add an entry under `Unreleased` in [CHANGELOG.md](CHANGELOG.md) for
   user-visible changes.
6. Run the formatting, test, and Clippy commands above.
7. Complete the pull request template, including hardware test results when
   applicable.

Hardware testing is not required for pure documentation or isolated host-side
logic, but display, input, launcher, and systemd changes should be tested on a
reMarkable 2 whenever possible. State the exact OS and rm2display versions in
the pull request.

## Style

- Prefer plain Rust and small modules over new dependencies.
- Preserve the single-threaded display/input state machine unless a change
  clearly requires otherwise.
- Keep model-provider behavior OpenAI-compatible rather than tied to one
  vendor.
- Keep visible language and animation consistent with the diary theme, while
  keeping logs and configuration names practical.
- Never log credentials or full authorization headers.

## Licensing

By contributing, you agree that your contribution may be distributed under
the project's MIT License. Do not submit code, fonts, images, or other assets
that the project cannot legally redistribute.
