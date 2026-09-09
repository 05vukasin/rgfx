# AUR packaging

Templates for publishing `rgfx` to the [Arch User Repository](https://aur.archlinux.org/).
Publishing is manual and owner-gated; these files are starting points, not
submitted packages.

| Package    | Source                        | Use when                                  |
| ---------- | ----------------------------- | ----------------------------------------- |
| `rgfx`     | tagged release source tarball | build the stable release from source      |
| `rgfx-bin` | prebuilt release binary       | fastest install, no Rust toolchain needed |
| `rgfx-git` | `main` branch via git         | track the latest development build        |

`rgfx-bin` and `rgfx-git` both `provides`/`conflicts` `rgfx`, so only one may be
installed at a time.

## Preparing a release

1. Bump `pkgver` in the relevant `PKGBUILD` to match the release tag (without the
   leading `v`).
2. Refresh checksums against the published artifacts:

   ```bash
   cd packaging/aur/rgfx      # or rgfx-bin
   updpkgsums                 # replaces the 'SKIP' placeholders
   ```

   For `rgfx-bin`, the expected checksums are the ones in the release
   `SHA256SUMS` file. `rgfx-git` intentionally keeps `SKIP` (VCS source).
3. Validate and generate `.SRCINFO`:

   ```bash
   makepkg --printsrcinfo > .SRCINFO
   namcap PKGBUILD          # optional lint
   ```
4. Build locally to smoke-test:

   ```bash
   makepkg -si
   ```
5. Commit `PKGBUILD` and `.SRCINFO` to the package's AUR git repository and push.

## Notes

- `arch` lists both `x86_64` and `aarch64`, matching the release archives.
- Licenses are `MIT OR Apache-2.0`; the source builds install both license files.
- `optdepends=('ffmpeg')` because video support shells out to `ffmpeg`/`ffprobe`
  at runtime.
