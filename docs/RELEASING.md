# Releasing Horcrux

Horcrux uses [Semantic Versioning](https://semver.org/):

- `MAJOR` changes incompatible user-facing behavior or on-tablet formats.
- `MINOR` adds backward-compatible features.
- `PATCH` fixes backward-compatible defects.

Before 1.0, a minor release may contain a necessary breaking change. Such a
change must be called out prominently in the changelog and release notes.

## Sources of version information

`Cargo.toml` is the primary version source. The same version must also appear
in:

- `packaging/appload/external.manifest.json`
- The dated release section in `CHANGELOG.md`
- The annotated Git tag, prefixed with `v`

`scripts/check-version.sh` verifies these values.

## Release checklist

1. Start from a clean, current `main` branch.
2. Update the package and AppLoad manifest versions.
3. Move user-visible changes from `Unreleased` into a dated changelog section.
4. Update the changelog comparison links.
5. Run:

   ```sh
   scripts/check-version.sh vX.Y.Z
   cargo fmt --all -- --check
   cargo test --locked --all-targets
   cargo clippy --locked --all-targets -- -D warnings
   scripts/build.sh
   scripts/package-release.sh
   ```

6. Commit the release preparation.
7. Create and push an annotated tag:

   ```sh
   git tag -a vX.Y.Z -m "Horcrux vX.Y.Z"
   git push origin main
   git push origin vX.Y.Z
   ```

The release workflow validates the tag, rebuilds the ARMv7 binary, creates a
reproducible archive and SHA-256 checksum, extracts the matching changelog
section, and publishes the GitHub release.

Do not move or recreate a published release tag. Fix a release problem with a
new patch version.
