# Patched eframe 0.36.2

eframe 0.36.2 from crates.io, used through `[patch.crates-io]` in `app/Cargo.toml`. Every change is
marked `PATCHED (biocartoon)` and only affects the web build's typing on phones. eframe 0.37.0 has
neither fix, so check a newer release for them before you drop this copy.

- `src/web/events.rs`, `touchend`: runs the app's frame while the tap is still being handled, plus the
  next frame if the tap gave a widget keyboard focus. This lets the text agent take focus inside the
  tap, so the keyboard comes up on the first tap of a text field instead of the second.
- `src/web/text_agent.rs` and `src/web/events.rs` (`should_prevent_default_for_key`): the hidden
  `<input>` always holds a zero-width space before the caret, and Backspace is no longer
  `preventDefault`ed while that input has focus. Backspace then always deletes something, so a held
  Backspace keeps repeating on iOS instead of stopping after one character, and Android keyboards
  send their delete.
