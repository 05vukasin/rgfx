# 025 — Stdin & pipe streaming

- **Crate:** `rgfx-cli`
- **Depends on:** 020, 008
- **Blocks:** —
- **Branch:** `task/025-cli-stdin-streaming`
- **Skills:** `finish-task`

## Goal
Support Unix-pipe input so `cat image.png | rgfx -` works, and lay groundwork for a
frame-stream mode (`some_generator | rgfx --stream`) per the spec's streaming section.

## Scope / deliverables
1. `-` as the file argument reads bytes from stdin, sniffs the format (magic bytes), and routes
   to the still-image viewer (021) — buffering the whole stdin for image decode.
2. `--stream` mode: read a simple framed protocol from stdin (documented: e.g. a small header
   `WxH` then raw RGB frames, or newline-delimited PNGs) and render continuously via the frame
   engine as a `FrameSource`. Keep the protocol minimal and documented in the CLI help + README.
3. Correct handling when stdin is a TTY / empty / closed mid-stream (clean exit).

## Contracts / API
Stdin is a `FrameSource`/byte source; reuse existing viewers + framebuffer. Cleanup guaranteed.

## Acceptance criteria
- [ ] `cat some.png | rgfx -` renders the image (test via piping bytes to the non-interactive path).
- [ ] `--stream` renders frames from the documented protocol (test with a synthetic byte stream).
- [ ] Empty/closed stdin exits cleanly, no hang/panic. Clippy `-D warnings`.

## Tests required
- [ ] Stdin image sniff + decode from an in-memory byte buffer → snapshot.
- [ ] `--stream` protocol parsing → correct frame count/sizes from a synthetic stream.
- [ ] EOF/partial-frame handling.

## Out of scope
Named pipes / Unix sockets / child-process sources (future). Programmatic Rust producer API (part of 021/public API work).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
