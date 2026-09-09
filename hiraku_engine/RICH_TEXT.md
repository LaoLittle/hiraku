# Ruby text

Ruby is presentation markup, not HKS syntax. The string/template is evaluated
normally before the engine parses the resulting markup:

```hks
"A {ruby:reading}word{/ruby}."

import ui.widgets.*
canvas {
    richText(dialogue.text).reveal(dialogue.revealedCharacters)
}
```

`text()` stays literal. Use `richText()` for dialogue, history and other text
that should interpret ruby. Omitting `.reveal()` displays the complete text.
Use `{{` and `}}` for literal braces. Ruby requires a nonempty single-line
reading and base; nested ruby is rejected. Other brace sequences stay literal.

The printer counts base characters only. It retains the full shaped layout
while revealing glyphs, including shadows. The reading appears above its base
when the first base character becomes visible. Appended dialogue retains
existing spans; markup remains in the dialogue/history model and save state.

Rendering uses ordinary Bevy text, spans and glyph layout. Reading text is half
the base font size, centered over the shaped base. Ruby bases stay together at
line breaks. Long readings may overhang; automatic reading-width compression
and `fitText()` for rich text are not implemented. Visual verification belongs
to the application's font and layout testing.
