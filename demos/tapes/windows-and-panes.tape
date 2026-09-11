Output demos/assets/windows-and-panes.gif

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

Type "./target/release/rustmux new-session -s rustmux-readme-layout"
Enter
Wait+Screen /Rustmux/
Sleep 1s

Ctrl+B
Type ","
Type@90ms "shell"
Enter
Sleep 700ms

Ctrl+B
Type "c"
Sleep 700ms
Ctrl+B
Type ","
Type@90ms "editor"
Enter
Sleep 900ms

Ctrl+B
Ctrl+P
Type "r"
Sleep 900ms
Type@45ms "echo 'right pane · cargo test'"
Enter
Sleep 900ms

Ctrl+B
Ctrl+P
Type "h"
Escape
Type@45ms "echo 'left pane · cargo run --release'"
Enter
Sleep 1s

Ctrl+B
Ctrl+P
Type "f"
Sleep 1s
Ctrl+B
Ctrl+P
Type "f"
Sleep 2s

Hide
Ctrl+B
Ctrl+O
Type "d"
Sleep 500ms
