Output demos/assets/session-manager.gif

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
Set TypingSpeed 50ms
Set WaitTimeout 10s

Hide
Type "clear"
Enter
Show

Type "./target/release/rustmux new-session -s rustmux-readme-main"
Enter
Wait+Screen /Rustmux/
Sleep 900ms

Ctrl+B
Ctrl+W
Sleep 900ms
Type "/"
Type@90ms "rustmux-readme-work"
Sleep 800ms
Enter
Wait+Screen /rustmux-readme-work/
Sleep 1s

Ctrl+B
Ctrl+W
Sleep 900ms
Type "/"
Type@90ms "main"
Sleep 2s

Escape
Escape
Sleep 700ms

Hide
Ctrl+B
Ctrl+O
Type "d"
Sleep 500ms
