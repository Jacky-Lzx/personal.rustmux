Output demos/assets/history-and-help.gif

Require fish
Set Shell "fish"
Set Theme "Catppuccin Mocha"
Set FontFamily "Cascadia Mono"
Set FontSize 18
Set LetterSpacing 0
Set Width 1200
Set Height 650
Set Padding 16
Set Margin 18
Set MarginFill "#11111b"
Set BorderRadius 8
Set Framerate 24
Set TypingSpeed 45ms
Set WaitTimeout 10s

Hide
Type "clear"
Enter
Show

Type "./target/release/rustmux new-session -s rustmux-readme-history"
Enter
Wait+Screen /Rustmux/
Sleep 900ms

Ctrl+B
Type "?"
Sleep 2s
Type "c"
Sleep 1s

Type@35ms "for i in (seq 1 28); printf 'history line %02d  ·  rustmux scrollback\\n' $i; end"
Enter
Sleep 1s

Ctrl+B
Type "["
Sleep 600ms
PageUp
Sleep 900ms
Type "/"
Type@100ms "history line 08"
Enter
Sleep 2s
Type "n"
Sleep 1s
Escape
Sleep 1s

Hide
Ctrl+B
Type "d"
Sleep 500ms
