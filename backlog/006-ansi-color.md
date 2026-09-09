# 006 — ANSI color layer

- **Crate:** `rgfx-terminal`
- **Depends on:** 003 (and integrates with 004/005 encoders)
- **Blocks:** 021, 022, 023
- **Branch:** `task/006-ansi-color`
- **Skills:** `rgfx-architecture`, `finish-task`

## Goal
Add ANSI color support to the terminal layer: capability detection and encoding of per-cell
foreground/background color at three fidelity levels, wired into the `TerminalFrame` output.

## Scope / deliverables
1. A `ColorMode` enum: `None` (grayscale), `Ansi16`, `Ansi256`, `TrueColor`.
2. `Framebuffer::Color` → nearest 16 / 256-color index / 24-bit truecolor escape sequences.
3. Terminal capability detection (env `COLORTERM`, `TERM`) to auto-pick a default mode; allow
   explicit override.
4. A serializer turning a `TerminalFrame` (chars + per-cell fg/bg) into a batched ANSI byte
   stream with minimal escape churn (only emit SGR when color changes between adjacent cells).
5. Extend `BrailleEncoder`/`BlockEncoder`/`AsciiEncoder` to optionally attach per-cell color
   (coordinate with 004/005 authors — this task may need to land after them; if their encoders
   already expose a color hook, just implement the serializer + detection).

## Contracts / API
Keep color in `rgfx_core`'s `TerminalFrame` cell representation. The serializer lives in
`rgfx-terminal`. No stdout writes from the serializer — it returns bytes for the CLI to flush.

## Acceptance criteria
- [ ] TrueColor escape for a known RGB matches `\x1b[38;2;R;G;Bm`.
- [ ] 256-color and 16-color quantization map known colors to expected indices.
- [ ] Adjacent equal-color cells do not re-emit SGR (assert on byte output).
- [ ] Capability detection picks TrueColor when `COLORTERM=truecolor`, degrades otherwise.

## Tests required
- [ ] RGB→16/256/truecolor mapping tables.
- [ ] SGR-minimization on a row of mixed/repeated colors.
- [ ] Capability detection across representative env combos.

## Out of scope
The encoders themselves (004/005). Video/image color conversion (that's upstream in the fb).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
